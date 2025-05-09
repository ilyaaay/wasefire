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

fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let board = if std::env::var_os("CARGO_FEATURE_BOARD_DEVKIT").is_some() {
        "devkit"
    } else if std::env::var_os("CARGO_FEATURE_BOARD_DONGLE").is_some()
        || std::env::var_os("CARGO_FEATURE_BOARD_MAKERDIARY").is_some()
    {
        "dongle"
    } else {
        panic!("one of board-{{devkit,dongle,makerdiary}} must be enabled")
    };
    std::fs::copy(format!("board-{board}.x"), out.join("board.x")).unwrap();
    println!("cargo::rerun-if-changed=board-{board}.x");
    println!("cargo::rerun-if-changed=memory.x");
    println!("cargo::rustc-link-search={}", out.display());
}
