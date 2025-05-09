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

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::cursor::*;
use crate::parser::{SkipData, SkipElem};
use crate::side_table::*;
use crate::syntax::*;
use crate::toctou::*;
use crate::*;

/// Valid module.
#[derive(Debug)]
pub struct Module<'m> {
    binary: &'m [u8],
    types: Vec<FuncType<'m>>,
    side_table: SideTableView<'m>,
}

impl<'m> Import<'m> {
    pub fn type_(&self, module: &Module<'m>) -> ExternType<'m> {
        self.desc.type_(module)
    }
}

impl ImportDesc {
    pub fn type_<'m>(&self, module: &Module<'m>) -> ExternType<'m> {
        match *self {
            ImportDesc::Func(x) => ExternType::Func(module.types[x as usize]),
            ImportDesc::Table(t) => ExternType::Table(t),
            ImportDesc::Mem(t) => ExternType::Mem(t),
            ImportDesc::Global(t) => ExternType::Global(t),
        }
    }
}

impl<'m> Module<'m> {
    /// Validates a WASM module in binary format.
    pub fn new(binary: &'m [u8]) -> Result<Self, Error> {
        crate::valid::verify(binary)?;
        Ok(unsafe { Self::new_unchecked(binary) })
    }

    /// Creates a valid module from binary format.
    ///
    /// # Safety
    ///
    /// The module must be valid.
    pub unsafe fn new_unchecked(binary: &'m [u8]) -> Self {
        // Only keep the sections (i.e. skip the header).
        let mut parser = unsafe { Parser::new(&binary[8 ..]) };
        let side_table = parser.parse_side_table().into_ok();
        let mut module = Module { binary: parser.remaining(), types: Vec::new(), side_table };
        if let Some(mut parser) = module.section(SectionId::Type) {
            for _ in 0 .. parser.parse_vec().into_ok() {
                module.types.push(parser.parse_functype().into_ok());
            }
        }
        module
    }

    pub(crate) fn types(&self) -> &[FuncType<'m>] {
        &self.types
    }

    pub(crate) fn imports(&self) -> impl Iterator<Item = Import<'m>> + use<'m> {
        let (n, mut parser) = match self.section(SectionId::Import) {
            None => (0, Parser::default()),
            Some(mut parser) => (parser.parse_vec().into_ok(), parser),
        };
        (0 .. n).map(move |_| {
            let module = parser.parse_name().into_ok();
            let name = parser.parse_name().into_ok();
            let desc = parser.parse_importdesc().into_ok();
            Import { module, name, desc }
        })
    }
}

pub type Parser<'m> = parser::Parser<'m, Use>;

impl<'m> Module<'m> {
    pub(crate) fn section(&self, expected_id: SectionId) -> Option<Parser<'m>> {
        let mut parser = unsafe { Parser::new(self.binary) };
        loop {
            if parser.is_empty() {
                return None;
            }
            let actual_id = parser.parse_section_id().into_ok();
            let section = parser.split_section().into_ok();
            if actual_id == SectionId::Custom {
                continue;
            }
            break match actual_id.order().cmp(&expected_id.order()) {
                Ordering::Less => continue,
                Ordering::Equal => Some(section),
                Ordering::Greater => None,
            };
        }
    }

    pub(crate) fn func_type(&self, x: FuncIdx) -> FuncType<'m> {
        self.types
            [self.side_table.metadata::<Use>(x as usize).into_ok().type_idx::<Use>().into_ok()]
    }

    pub(crate) fn table_type(&self, x: TableIdx) -> TableType {
        let mut parser = self.section(SectionId::Table).unwrap();
        for i in 0 .. parser.parse_vec().into_ok() {
            let t = parser.parse_tabletype().into_ok();
            if i == x as usize {
                return t;
            }
        }
        unreachable!()
    }

    pub(crate) fn mem_type(&self, x: MemIdx) -> MemType {
        let mut parser = self.section(SectionId::Memory).unwrap();
        for i in 0 .. parser.parse_vec().into_ok() {
            let t = parser.parse_memtype().into_ok();
            if i == x as usize {
                return t;
            }
        }
        unreachable!()
    }

    pub(crate) fn global_type(&self, x: GlobalIdx) -> GlobalType {
        let mut parser = self.section(SectionId::Global).unwrap();
        for i in 0 .. parser.parse_vec().into_ok() {
            let t = parser.parse_globaltype().into_ok();
            parser.skip_to_end(0).into_ok();
            if i == x as usize {
                return t;
            }
        }
        unreachable!()
    }

    pub(crate) fn export(&self, expected_name: &str) -> Option<ExportDesc> {
        let mut parser = self.section(SectionId::Export).unwrap();
        for _ in 0 .. parser.parse_vec().into_ok() {
            let actual_name = parser.parse_name().into_ok();
            let desc = parser.parse_exportdesc().into_ok();
            if actual_name == expected_name {
                return Some(desc);
            }
        }
        None
    }

    pub(crate) fn elem(&self, x: ElemIdx) -> Parser<'m> {
        let mut parser = self.section(SectionId::Element).unwrap();
        for i in 0 .. parser.parse_vec().into_ok() {
            if i == x as usize {
                return parser;
            }
            parser.parse_elem(&mut SkipElem).into_ok();
        }
        unreachable!()
    }

    pub(crate) fn func(&self, x: FuncIdx) -> (Parser<'m>, Cursor<'m, BranchTableEntry>) {
        let metadata = self.side_table.metadata::<Use>(x as usize).into_ok();
        let mut parser = unsafe { Parser::new(self.binary) };
        unsafe { parser.restore(metadata.parser_state::<Use>().into_ok()) };
        (parser, Cursor::new(metadata.branch_table::<Use>().into_ok()))
    }

    pub(crate) fn data(&self, x: DataIdx) -> Parser<'m> {
        let mut parser = self.section(SectionId::Data).unwrap();
        for i in 0 .. parser.parse_vec().into_ok() {
            if i == x as usize {
                return parser;
            }
            parser.parse_data(&mut SkipData).into_ok();
        }
        unreachable!()
    }
}
