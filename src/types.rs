// SPDX-License-Identifier: EUPL-1.2
//! the shared owned model lives in `parsers::manpage`. only `Positional`,
//! shared by both layers, remains here.

#[derive(Debug, Clone)]
pub struct Positional {
    pub optional: bool,
    pub variadic: bool,
}
