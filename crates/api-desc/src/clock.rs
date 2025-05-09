// Copyright 2024 Google LLC
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

use crate::*;

pub(crate) fn new() -> Item {
    let docs = docs! {
        /// Clock operations.
    };
    let name = "clock".into();
    let items = vec![item! {
        /// Returns the time spent since some initial event, in micro-seconds.
        ///
        /// The initial event may be the first time this function is called.
        fn uptime_us "clk" {
            /// Pointer to the 64-bits time.
            ptr: *mut u64,
        } -> ()
    }];
    Item::Mod(Mod { docs, name, items })
}
