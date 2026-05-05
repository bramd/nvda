//! NVDA IA2: Hand-rolled bindings for the IAccessible2 family of COM
//! interfaces, plus Rust ports of selected helpers from `nvdaHelper/common/
//! ia2utils.cpp`.
//!
//! This crate is built as a `staticlib` and linked into
//! `nvdaHelperRemote.dll`, which is injected into target processes (browsers,
//! Office apps, etc.). Keep dependencies minimal and avoid host-process
//! global state.
//!
//! Bindings are hand-rolled (not generated from the IDLs in `include/ia2/api/`)
//! so the crate has no MIDL dependency. IIDs and method orderings are copied
//! verbatim from those IDLs — keep them in sync if the submodule updates.

#![allow(non_snake_case)]
// Bindings for interfaces not yet exercised in this PR — will be used by
// the textFromIAccessible port in the follow-up PR.
#![allow(dead_code)]

pub mod acc_description;
pub mod attribs;
pub mod child_count;
pub mod details_roles;
pub mod fetch;
pub mod find_descendant;
pub mod from_identifier;
pub mod hyperlink_getter;
pub mod interfaces;
pub mod label_info;
pub mod live_regions;
pub mod relation_targets;
pub mod role_long_string;
pub mod selected_item;
pub mod text;
pub mod textbox_in_combobox;
pub mod types;
