//! Identifier newtypes: what a string *is*, checked by the compiler
//! instead of remembered by the reader.

use derive_more::{AsRef, Display, From};

/// A reference designator ("R101"): a part's identity across revisions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct Reference(String);

/// A pin number on a part ("17", "A1"), meaningful only next to a [`Reference`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct Pin(String);

/// A net name as exported ("/Power Switch/GATE"); unescape via `short_net`
/// before showing it to a person.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct NetName(String);

/// The name of a report page - a schematic sheet ("Root sheet") or a board
/// layer ("F.Cu"). Changes navigate by carrying the page name they belong to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct PageName(String);

/// A project's sidebar label (its repo-relative dir, or its bare name).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct ProjectLabel(String);

/// A part's identity key: the footprint UUID on boards, instance path plus
/// symbol UUID on schematics.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Display, From, AsRef)]
#[from(forward)]
#[as_ref(str)]
pub struct EntityId(String);
