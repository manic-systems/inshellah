// SPDX-License-Identifier: EUPL-1.2
//! The shared owned model lives in `parsers::manpage` (`OwnedSwitch`,
//! `OwnedParam`, `ManpageEntry`, `ManpageSubcommand`, `ManpageResult`). Both
//! the help and manpage parsers now produce it directly; the borrowed
//! intermediates they used to convert from are private to the option parser.
//! Only `Positional` — shared by both layers — remains here.

#[derive(Debug, Clone)]
pub struct Positional {
    pub optional: bool,
    pub variadic: bool,
}
