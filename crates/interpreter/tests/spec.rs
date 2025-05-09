// Copyright 2022 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![feature(int_roundings)]
#![allow(unused_crate_dependencies)]

use std::collections::HashMap;

use lazy_static::lazy_static;
use wasefire_interpreter::*;
use wast::core::{AbstractHeapType, WastArgCore, WastRetCore};
use wast::lexer::Lexer;
use wast::token::Id;
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat, parser};

fn test(repo: &str, name: &str, skip: usize) {
    let path = format!("../../third_party/WebAssembly/{repo}/test/core/{name}.wast");
    let mut content = std::fs::read_to_string(path).unwrap();
    patch_content(name, &mut content);
    let mut lexer = Lexer::new(&content);
    lexer.allow_confusing_unicode(true);
    let buffer = parser::ParseBuffer::new_with_lexer(lexer).unwrap();
    let wast: Wast = parser::parse(&buffer).unwrap();
    let layout = std::alloc::Layout::from_size_align(pool_size(name), MEMORY_ALIGN).unwrap();
    let pool = unsafe { std::slice::from_raw_parts_mut(std::alloc::alloc(layout), layout.size()) };
    let mut env = Env::new(pool);
    env.instantiate("spectest", &SPECTEST);
    env.register_name("spectest", None);
    assert!(matches!(env.inst, Sup::Yes(_)));
    for directive in wast.directives {
        eprintln!("{name}:{}", directive.span().offset());
        match directive {
            WastDirective::Module(QuoteWat::Wat(Wat::Module(mut m))) => {
                env.instantiate(name, &m.encode().unwrap());
                env.register_id(m.id, env.inst);
            }
            WastDirective::Module(mut wat) => env.instantiate(name, &wat.encode().unwrap()),
            WastDirective::AssertMalformed { module, .. } => assert_malformed(&mut env, module),
            WastDirective::AssertInvalid { module, .. } => assert_invalid(&mut env, module),
            WastDirective::AssertReturn { exec, results, .. } => {
                assert_return(&mut env, exec, results)
            }
            WastDirective::AssertTrap { exec, .. } => assert_trap(&mut env, exec),
            WastDirective::Invoke(invoke) => assert_invoke(&mut env, invoke),
            WastDirective::AssertExhaustion { call, .. } => assert_exhaustion(&mut env, call),
            WastDirective::Register { name, module, .. } => env.register_name(name, module),
            WastDirective::AssertUnlinkable { module, .. } => assert_unlinkable(&mut env, module),
            _ => unimplemented!("{:?}", directive),
        }
    }
    assert_eq!(env.skip, skip, "actual vs expected number of unsupported (and skipped) tests");
}

fn patch_content(name: &str, content: &mut String) {
    if name == "br_table" {
        // This is a corner-case we don't want to support.
        replace_with(content, "\n  (func (export \"large\")", "\n  )\n", "");
        replace_with(content, "\n(assert_return (invoke \"large\" ", "\n\n", "\n");
    }
}

fn replace_with(content: &mut String, prefix: &str, suffix: &str, replace: &str) {
    let start = content.find(prefix).unwrap();
    let length = content[start ..].find(suffix).unwrap() + suffix.len();
    content.replace_range(start .. start + length, replace);
}

fn pool_size(name: &str) -> usize {
    match name {
        "address" => 0x200000,
        "align" => 0x200000,
        "bulk" => 0x200000,
        "const" => 0x200000,
        "data" => 0x400000,
        "linking" => 0x1000000,
        "memory_copy" => 0x400000,
        "memory_fill" => 0x200000,
        "memory_grow" => 0x800000,
        "memory_init" => 0x400000,
        "memory_trap" => 0x200000,
        _ => 0x100000,
    }
}

fn mem_size(name: &str) -> usize {
    match name {
        "address" => 0x10000,
        "align" => 0x10000,
        "bulk" => 0x10000,
        "data" => 0x20000,
        "linking" => 0x60000,
        "memory" => 0x10000,
        "memory_copy" => 0x10000,
        "memory_fill" => 0x10000,
        "memory_grow" => 0x80000,
        "memory_init" => 0x20000,
        "memory_trap" => 0x10000,
        "spectest" => 0x10000,
        _ => 0x1000,
    }
}

/// Whether something is supported.
#[derive(Copy, Clone)]
enum Sup<T> {
    Uninit,
    No(Unsupported),
    Yes(T),
}

impl<T> Sup<T> {
    fn conv(x: Result<T, Error>) -> Result<Sup<T>, Error> {
        match x {
            Ok(x) => Ok(Sup::Yes(x)),
            Err(Error::Unsupported(x)) => {
                eprintln!("unsupported {x:?}");
                Ok(Sup::No(x))
            }
            Err(x) => Err(x),
        }
    }

    fn res(self) -> Result<T, Error> {
        match self {
            Sup::Uninit => unreachable!(),
            Sup::No(x) => Err(Error::Unsupported(x)),
            Sup::Yes(x) => Ok(x),
        }
    }
}

macro_rules! only_sup {
    ($e:expr, $x:expr) => {
        match $x {
            Ok(x) => Ok(x),
            Err(Error::Unsupported(x)) => {
                eprintln!("skip unsupported {x:?}");
                $e.skip += 1;
                return;
            }
            Err(x) => Err(x),
        }
    };
}

struct Env<'m> {
    pool: &'m mut [u8],
    store: Store<'m>,
    inst: Sup<InstId>,
    map: HashMap<Id<'m>, Sup<InstId>>,
    skip: usize,
}

impl<'m> Env<'m> {
    fn new(pool: &'m mut [u8]) -> Self {
        Env { pool, store: Store::default(), inst: Sup::Uninit, map: HashMap::new(), skip: 0 }
    }

    fn alloc(&mut self, size: usize) -> &'m mut [u8] {
        if self.pool.len() < size {
            panic!("pool is too small");
        }
        let mut self_pool: &mut [u8] = &mut [];
        std::mem::swap(&mut self.pool, &mut self_pool);
        let (result, pool) = self_pool.split_at_mut(size.next_multiple_of(MEMORY_ALIGN));
        self.pool = pool;
        &mut result[.. size]
    }

    fn maybe_instantiate(&mut self, name: &str, wasm: &[u8]) -> Result<InstId, Error> {
        let wasm = prepare(wasm)?;
        let module = self.alloc(wasm.len());
        module.copy_from_slice(&wasm);
        let module = Module::new(module)?;
        let memory = self.alloc(mem_size(name));
        self.store.instantiate(module, memory)
    }

    fn instantiate(&mut self, name: &str, wasm: &[u8]) {
        let inst = self.maybe_instantiate(name, wasm);
        self.inst = Sup::conv(inst).unwrap();
    }

    fn invoke(&mut self, inst_id: InstId, name: &str, args: Vec<Val>) -> Result<Vec<Val>, Error> {
        Ok(match self.store.invoke(inst_id, name, args)? {
            RunResult::Done(x) => x,
            RunResult::Host { .. } => unreachable!(),
        })
    }

    fn register_name(&mut self, name: &'m str, module: Option<Id<'m>>) {
        self.register_id(module, self.inst);
        if let Sup::Yes(inst_id) = self.inst {
            self.store.set_name(inst_id, name).unwrap();
        }
    }

    fn register_id(&mut self, id: Option<Id<'m>>, inst_id: Sup<InstId>) {
        if let Some(id) = id {
            self.map.insert(id, inst_id);
        }
    }

    fn inst_id(&self, id: Option<Id>) -> Sup<InstId> {
        match id {
            Some(x) => self.map[&x],
            None => self.inst,
        }
    }
}

lazy_static! {
    static ref SPECTEST: Vec<u8> = spectest();
}

#[allow(clippy::vec_init_then_push)]
fn spectest() -> Vec<u8> {
    let leb128 = |wasm: &mut Vec<u8>, mut x: usize| {
        assert!(x <= u32::MAX as usize);
        while x > 127 {
            wasm.push(0x80 | (x & 0x7f) as u8);
            x >>= 7;
        }
        wasm.push(x as u8);
    };

    let types: &[&[u8]] = &[
        b"\x60\x00\x00",         // () -> ()
        b"\x60\x01\x7f\x00",     // (i32) -> ()
        b"\x60\x01\x7e\x00",     // (i64) -> ()
        b"\x60\x01\x7d\x00",     // (f32) -> ()
        b"\x60\x01\x7c\x00",     // (f64) -> ()
        b"\x60\x02\x7f\x7d\x00", // (i32, f32) -> ()
        b"\x60\x02\x7c\x7c\x00", // (f64, f64) -> ()
    ];
    let functions: Vec<_> = (0 .. types.len()).map(|x| x as u8).collect();
    let functions: Vec<_> = functions.iter().map(std::slice::from_ref).collect();
    let table = &[0x70, 1, 10, 20]; // funcref { min 10, max 20 }
    let memory = &[1, 1, 2]; // { min 1, max 2 }
    let mut globals: Vec<&[u8]> = Vec::new();
    globals.push(b"\x7f\x00\x41\x9a\x05\x0b"); // { type i32 const, init 666 }
    globals.push(b"\x7e\x00\x42\x9a\x05\x0b"); // { type i64 const, init 666 }
    #[cfg(feature = "float-types")] // { type f32 const, init 666.6 }
    globals.push(b"\x7d\x00\x43\x66\xa6\x26\x44\x0b");
    #[cfg(feature = "float-types")] // { type f64 const, init 666.6 }
    globals.push(b"\x7c\x00\x44\xcd\xcc\xcc\xcc\xcc\xd4\x84\x40\x0b");
    let export = |name: &str, desc: u8, idx: usize| {
        assert!(name.is_ascii());
        let mut wasm = Vec::new();
        leb128(&mut wasm, name.len());
        wasm.extend_from_slice(name.as_bytes());
        wasm.push(desc);
        leb128(&mut wasm, idx);
        wasm
    };
    let mut exports = Vec::new();
    exports.push(export("print", 0, 0));
    exports.push(export("print_i32", 0, 1));
    exports.push(export("print_i64", 0, 2));
    exports.push(export("print_f32", 0, 3));
    exports.push(export("print_f64", 0, 4));
    exports.push(export("print_i32_f32", 0, 5));
    exports.push(export("print_f64_f64", 0, 6));
    exports.push(export("table", 1, 0));
    exports.push(export("memory", 2, 0));
    exports.push(export("global_i32", 3, 0));
    exports.push(export("global_i64", 3, 1));
    #[cfg(feature = "float-types")]
    exports.push(export("global_f32", 3, 2));
    #[cfg(feature = "float-types")]
    exports.push(export("global_f64", 3, 3));
    let exports: Vec<_> = exports.iter().map(Vec::as_slice).collect();
    let codes: Vec<_> = functions.iter().map(|_| &b"\x02\x00\x0b"[..]).collect();

    let section = |wasm: &mut Vec<u8>, id: u8, xs: &[&[u8]]| {
        assert!(id <= 12);
        wasm.push(id);
        leb128(wasm, xs.iter().map(|x| x.len()).sum::<usize>() + 1);
        assert!(xs.len() < 128);
        wasm.push(xs.len() as u8);
        for x in xs {
            wasm.extend_from_slice(x);
        }
    };
    let mut wasm = Vec::new();
    wasm.extend_from_slice(b"\0asm\x01\0\0\0");
    section(&mut wasm, 1, types);
    section(&mut wasm, 3, &functions);
    section(&mut wasm, 4, &[table]);
    section(&mut wasm, 5, &[memory]);
    section(&mut wasm, 6, &globals);
    section(&mut wasm, 7, &exports);
    section(&mut wasm, 10, &codes);
    wasm
}

fn assert_return(env: &mut Env, exec: WastExecute, expected: Vec<WastRet>) {
    let actual = only_sup!(env, wast_execute(env, exec)).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.into_iter().zip(expected.into_iter()) {
        use Val::*;
        use WastRet::Core as C;
        use WastRetCore as W;
        use wast::core::HeapType;
        #[cfg(feature = "float-types")]
        use wast::core::NanPattern as NP;
        match (actual, expected) {
            (I32(x), C(W::I32(y))) => assert_eq!(x, y as u32),
            (I64(x), C(W::I64(y))) => assert_eq!(x, y as u64),
            #[cfg(feature = "float-types")]
            (F32(x), C(W::F32(NP::Value(y)))) => assert_eq!(x, y.bits),
            #[cfg(feature = "float-types")]
            (F32(x), C(W::F32(_))) => assert!(f32::from_bits(x).is_nan()),
            #[cfg(feature = "float-types")]
            (F64(x), C(W::F64(NP::Value(y)))) => assert_eq!(x, y.bits),
            #[cfg(feature = "float-types")]
            (F64(x), C(W::F64(_))) => assert!(f64::from_bits(x).is_nan()),
            #[cfg(feature = "vector-types")]
            (V128(_), _) => unimplemented!(),
            (
                Null(RefType::ExternRef),
                C(W::RefNull(None | Some(HeapType::Abstract { ty: AbstractHeapType::Extern, .. }))),
            ) => (),
            (
                Null(RefType::FuncRef),
                C(W::RefNull(None | Some(HeapType::Abstract { ty: AbstractHeapType::Func, .. }))),
            ) => (),
            (Ref(_), _) => unimplemented!(),
            (RefExtern(x), C(W::RefExtern(Some(y)))) => assert_eq!(x, y as usize),
            (x, y) => panic!("{x:?} !~ {y:?}"),
        }
    }
}

fn assert_trap(env: &mut Env, exec: WastExecute) {
    assert_eq!(only_sup!(env, wast_execute(env, exec)), Err(Error::Trap));
}

fn assert_invoke(env: &mut Env, invoke: WastInvoke) {
    assert_eq!(only_sup!(env, wast_invoke(env, invoke)), Ok(Vec::new()));
}

fn assert_malformed(env: &mut Env, mut wat: QuoteWat) {
    if let Ok(wasm) = wat.encode() {
        assert_eq!(only_sup!(env, prepare(&wasm)).err(), Some(Error::Invalid));
    }
}

fn assert_invalid(env: &mut Env, mut wat: QuoteWat) {
    let wasm = wat.encode().unwrap();
    assert_eq!(only_sup!(env, prepare(&wasm)).err(), Some(Error::Invalid));
}

fn assert_exhaustion(env: &mut Env, call: WastInvoke) {
    assert_eq!(only_sup!(env, wast_invoke(env, call)), Err(Error::Trap));
}

fn assert_unlinkable(env: &mut Env, mut wat: Wat) {
    let inst = only_sup!(env, env.maybe_instantiate("", &wat.encode().unwrap()));
    assert_eq!(inst.err(), Some(Error::NotFound));
}

fn wast_execute(env: &mut Env, exec: WastExecute) -> Result<Vec<Val>, Error> {
    match exec {
        WastExecute::Invoke(invoke) => wast_invoke(env, invoke),
        WastExecute::Wat(mut wat) => {
            env.maybe_instantiate("", &wat.encode().unwrap()).map(|_| Vec::new())
        }
        WastExecute::Get { module, global, .. } => {
            let inst_id = env.inst_id(module).res()?;
            env.store.get_global(inst_id, global).map(|x| vec![x])
        }
    }
}

fn wast_invoke(env: &mut Env, invoke: WastInvoke) -> Result<Vec<Val>, Error> {
    let inst_id = env.inst_id(invoke.module).res()?;
    let args = wast_args(invoke.args);
    env.invoke(inst_id, invoke.name, args)
}

fn wast_args(args: Vec<WastArg>) -> Vec<Val> {
    args.into_iter().map(|arg| wast_arg(arg)).collect()
}

fn wast_arg(arg: WastArg) -> Val {
    match arg {
        WastArg::Core(core) => wast_arg_core(core),
        _ => unimplemented!("{:?}", arg),
    }
}

fn wast_arg_core(core: WastArgCore) -> Val {
    use wast::core::HeapType;
    match core {
        WastArgCore::I32(x) => Val::I32(x as u32),
        WastArgCore::I64(x) => Val::I64(x as u64),
        #[cfg(feature = "float-types")]
        WastArgCore::F32(x) => Val::F32(x.bits),
        #[cfg(feature = "float-types")]
        WastArgCore::F64(x) => Val::F64(x.bits),
        WastArgCore::RefNull(HeapType::Abstract { ty: AbstractHeapType::Func, .. }) => {
            Val::Null(RefType::FuncRef)
        }
        WastArgCore::RefNull(HeapType::Abstract { ty: AbstractHeapType::Extern, .. }) => {
            Val::Null(RefType::ExternRef)
        }
        WastArgCore::RefExtern(x) => Val::RefExtern(x as usize),
        _ => unimplemented!("{:?}", core),
    }
}

macro_rules! test {
    ($(#[$m:meta])* $($repo:literal,)? $name:ident$(, $file:literal)?$(; $skip:literal)?) => {
        test!(=1 {$(#[$m])*} [$($repo)?] $name [$($file)?] [$($skip)?]);
    };
    (=1 $meta:tt [] $name:ident $file:tt $skip:tt) => {
        test!(=2 $meta "spec" $name $file $skip);
    };
    (=1 $meta:tt [$repo:literal] $name:ident $file:tt $skip:tt) => {
        test!(=2 $meta $repo $name $file $skip);
    };
    (=2 $meta:tt $repo:literal $name:ident [] $skip:tt) => {
        test!(=3 $meta $repo $name $name $skip);
    };
    (=2 $meta:tt $repo:literal $name:ident [$file:literal] $skip:tt) => {
        test!(=3 $meta $repo $name $file $skip);
    };
    (=3 $meta:tt $repo:literal $name:ident $file:tt []) => {
        test!(=4 $meta $repo $name $file 0);
    };
    (=3 $meta:tt $repo:literal $name:ident $file:tt [$skip:literal]) => {
        test!(=4 $meta $repo $name $file $skip);
    };
    (=4 {$(#[$m:meta])*} $repo:literal $name:ident $file:tt $skip:literal) => {
        #[test] $(#[$m])* fn $name() { test($repo, test!(=5 $file), $skip); }
    };
    (=5 $name:ident) => { stringify!($name) };
    (=5 $file:literal) => { $file };
}

test!(address);
test!(align);
test!(binary);
test!(binary_leb128, "binary-leb128");
test!(block);
test!(br);
test!(br_if);
test!(br_table);
test!(bulk);
test!(call);
test!(call_indirect);
test!(comments);
test!(const_, "const");
test!(conversions);
test!(custom);
test!(data);
test!(elem);
test!(endianness);
test!(exports);
test!(f32);
test!(f32_bitwise);
test!(f32_cmp);
test!(f64);
test!(f64_bitwise);
test!(f64_cmp);
test!(fac);
test!(float_exprs);
test!(float_literals);
test!(float_memory);
test!(float_misc);
test!(forward);
test!(func);
test!(func_ptrs);
test!(global);
test!(i32);
test!(i64);
test!(if_, "if");
test!(imports);
test!(inline_module, "inline-module");
test!(int_exprs);
test!(int_literals);
test!(labels);
test!(left_to_right, "left-to-right");
test!(linking);
test!(load);
test!(local_get);
test!(local_set);
test!(local_tee);
test!(loop_, "loop");
test!(memory);
test!(memory_copy);
test!(memory_fill);
test!(memory_grow);
test!(memory_init);
test!(memory_redundancy);
test!(memory_size);
test!(memory_trap);
test!(names);
test!(nop);
test!(obsolete_keywords, "obsolete-keywords");
test!(ref_func);
test!(ref_is_null);
test!(ref_null);
test!(return_, "return");
test!(select);
test!(skip_stack_guard_page, "skip-stack-guard-page"; 10);
test!(stack);
test!(start);
test!(store);
test!(switch);
test!(table);
test!(table_sub, "table-sub");
test!(table_copy);
test!(table_fill);
test!(table_get);
test!(table_grow);
test!(table_init);
test!(table_set);
test!(table_size);
test!(token);
test!(traps);
test!(type_, "type");
test!(unreachable);
test!(unreached_invalid, "unreached-invalid");
test!(unreached_valid, "unreached-valid");
test!(unwind);
test!(utf8_custom_section_id, "utf8-custom-section-id");
test!(utf8_import_field, "utf8-import-field");
test!(utf8_import_module, "utf8-import-module");
test!(utf8_invalid_encoding, "utf8-invalid-encoding");
