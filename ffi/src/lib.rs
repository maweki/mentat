// Copyright 2018 Mozilla
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not use
// this file except in compliance with the License. You may obtain a copy of the
// License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
// CONDITIONS OF ANY KIND, either express or implied. See the License for the
// specific language governing permissions and limitations under the License.

//! This module exposes an Foreign Function Interface (FFI) that allows Mentat to be
//! called from other languages.
//!
//! Functions that are available to other languages in this module are defined as
//! extern "C" functions which allow them to be layed out correctly for the
//! platform's C ABI. They all have a `#[no_mangle]` decorator to ensure
//! Rust's name mangling is turned off, so that it is easier to link to.
//!
//! Mentat's FFI contains unsafe code. As it is an interface between foreign code
//! and native Rust code, Rust cannot guarantee that the types and data that have been passed
//! to it from another language are present and in the format it is expecting.
//! This interface is designed to ensure that nothing unsafe passes through this module
//! and enters Mentat proper
//!
//! Structs defined with `#[repr(C)]` are guaranteed to have a layout that is compatible
//! with the platform's representation in C.
//!
//! This API passes pointers in two ways, depending on the lifetime of the value and
//! what value owns it.
//! Pointers to values that are guaranteed to live beyond the lifetime of the function,
//! are passed over the FFI as a raw pointer.
//!
//! `value as *const Binding`
//!
//! Pointers to values that cannot be guaranteed to live beyond the lifetime of the function
//! are first `Box`ed so that they live on the heap, and the raw pointer passed this way.
//!
//! `Box::into_raw(Box::new(value))`
//!
//! The memory for a value that is moved onto the heap before being passed over the FFI
//! is no longer managed by Rust, but Rust still owns the value. Therefore the pointer
//! must be returned to Rust in order to be released. To this effect a number of `destructor`
//! functions are provided for each Rust value type that is passed, as is a catch all destructor
//! to release memory for `#[repr(C)]` values.
//! The destructors reclaim the memory via [Box](std::boxed::Box) and then drop the reference, causing the
//! memory to be released.
//!
//! A macro has been provided to make defining destructors easier.
//!
//! `define_destructor!(query_builder_destroy, QueryBuilder);`
//!
//! Passing a pointer to memory that has already been released will cause Mentat to crash,
//! so callers have to be careful to ensure they manage their pointers properly.
//! Failure to call a destructor for a value on the heap will cause a memory leak.
//!
//! Generally, the functions exposed in this module have a direct mapping to existing Mentat APIs,
//! in order to keep application logic to a minumum and provide the greatest flexibility
//! for callers using the interface. However, in some cases a single convenience function
//! has been provided in order to make the interface easier to use and reduce the number
//! of calls that have to be made over the FFI to perform a task. An example of this is
//! `store_register_observer`, which takes a single native callback function that is then
//! wrapped inside a Rust closure and added to a [TxObserver](mentat::TxObserver) struct. This is then used to
//! register the observer with the store.
//!
//! [Result](std::result::Result) and [Option](std::option::Option) Rust types have `repr(C)` structs that mirror them. This is to provide a more
//! native access pattern to callers and to enable easier passing of optional types and error
//! propogation. These types have implemented [From](std::convert::From) such that conversion from the Rust type
//! to the C type is as painless as possible.

extern crate libc;
extern crate mentat;

use std::collections::{
    BTreeSet,
};
use std::os::raw::{
    c_char,
    c_int,
    c_longlong,
    c_void,
};
use std::slice;
use std::sync::{
    Arc,
};
use std::vec;

pub use mentat::{
    Binding,
    CacheDirection,
    Entid,
    FindSpec,
    HasSchema,
    InProgress,
    KnownEntid,
    Queryable,
    QueryBuilder,
    QueryInputs,
    QueryOutput,
    QueryResults,
    RelResult,
    Store,
    Syncable,
    TypedValue,
    TxObserver,
    TxReport,
    Uuid,
    ValueType,
    Variable,
};

pub use mentat::entity_builder::{
    BuildTerms,
    EntityBuilder,
    InProgressBuilder,
    IntoThing,
};

pub mod android;
pub mod utils;

pub use utils::strings::{
    c_char_to_string,
    kw_from_string,
    string_to_c_char,
};

pub use utils::log;

// type aliases for iterator types.
pub type BindingIterator = vec::IntoIter<Binding>;
pub type BindingListIterator = std::slice::Chunks<'static, mentat::Binding>;

/// A C representation of the change provided by the transaction observers
/// from a single transact.
/// Holds a transaction identifier, the changes as a set of affected attributes
/// and the length of the list of changes.
///
/// #Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct TransactionChange {
    pub txid: Entid,
    pub changes_len: usize,
    pub changes: Box<[Entid]>,
}

 /// A C representation of the list of changes provided by the transaction observers.
 /// Provides the list of changes as the length of the list.
///
/// #Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[repr(C)]
#[derive(Debug)]
pub struct TxChangeList {
    pub reports: Box<[TransactionChange]>,
    pub len: usize,
}

/// A C representation Rust's [Option](std::option::Option).
/// A value of `Some` results in `value` containing a raw pointer as a `c_void`.
/// A value of `None` results in `value` containing a null pointer.
///
/// #Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[repr(C)]
#[derive(Debug)]
pub struct ExternOption {
    pub value: *mut c_void,
}

impl<T> From<Option<T>> for ExternOption {
    fn from(option: Option<T>) -> Self {
        ExternOption {
            value: option.map_or(std::ptr::null_mut(), |v| Box::into_raw(Box::new(v)) as *mut _ as *mut c_void)
        }
    }
}

/// A C representation Rust's [Result](std::result::Result).
/// A value of `Ok` results in `ok` containing a raw pointer as a `c_void`
/// and `err` containing a null pointer.
/// A value of `Err` results in `value` containing a null pointer and `err` containing an error message.
///
/// #Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[repr(C)]
#[derive(Debug)]
pub struct ExternResult {
    pub ok: *const c_void,
    pub err: *const c_char,
}

impl<T, E> From<Result<T, E>> for ExternResult where E: std::error::Error {
    fn from(result: Result<T, E>) -> Self {
        match result {
            Ok(value) => {
                ExternResult {
                    err: std::ptr::null(),
                    ok: Box::into_raw(Box::new(value)) as *const _ as *const c_void,
                }
            },
            Err(e) => {
                ExternResult {
                    err: string_to_c_char(e.to_string()),
                    ok: std::ptr::null(),
                }
            }
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct InProgressTransactResult<'a, 'c> {
    pub in_progress: *mut InProgress<'a, 'c>,
    pub result: *mut ExternResult,
}

/// A store cannot be opened twice to the same location.
/// Once created, the reference to the store is held by the caller and not Rust,
/// therefore the caller is responsible for calling `destroy` to release the memory
/// used by the [Store](mentat::Store) in order to avoid a memory leak.
// TODO: Start returning `ExternResult`s rather than crashing on error.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `store_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub extern "C" fn store_open(uri: *const c_char) -> *mut Store {
    let uri = c_char_to_string(uri);
    let store = Store::open(&uri).expect("expected a store");
    Box::into_raw(Box::new(store))
}

/// Variant of store_open that opens an encrypted database.
#[cfg(feature = "sqlcipher")]
#[no_mangle]
pub extern "C" fn store_open_encrypted(uri: *const c_char, key: *const c_char) -> *mut Store {
    let uri = c_char_to_string(uri);
    let key = c_char_to_string(key);
    let store = Store::open_with_key(&uri, &key).expect("expected a store");
    Box::into_raw(Box::new(store))
}

// TODO: open empty

// TODO: dismantle

// TODO: conn

// TODO: begin_read

/// Starts a new transaction to allow multiple transacts to be
/// performed together. This is more efficient than performing
/// a large set of individual commits.
///
/// Returns a [Result<TxReport>](mentat::TxReport) as an [ExternResult](ExternResult).
///
/// # Safety
///
/// Callers must ensure that the pointer to the [Store](mentat::Store) is not dangling.
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `tx_report_destroy` is provided for releasing the memory for this
/// pointer type.
///
/// TODO: Document the errors that can result from begin_transaction
#[no_mangle]
pub unsafe extern "C" fn store_begin_transaction(store: *mut Store) -> *mut ExternResult {
    let store = &mut *store;
    Box::into_raw(Box::new(store.begin_transaction().into()))
}

/// Perform a single transact operation using the current in progress
/// transaction. Takes edn as a string to transact.
///
/// Returns a [Result<TxReport>](mentat::TxReport) as an [ExternResult](ExternResult).
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `tx_report_destroy` is provided for releasing the memory for this
/// pointer type.
///
/// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn in_progress_transact<'m>(in_progress: *mut InProgress<'m, 'm>, transaction: *const c_char) -> *mut ExternResult {
    let in_progress = &mut *in_progress;
    let transaction = c_char_to_string(transaction);
    Box::into_raw(Box::new(in_progress.transact(transaction).into()))
}

/// Commit all the transacts that have been performed using this
/// in progress transaction.
///
/// Returns a [Result<()>](std::result::Result) as an [ExternResult](ExternResult).
///
/// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn in_progress_commit<'m>(in_progress: *mut InProgress<'m, 'm>) -> *mut ExternResult {
    let in_progress = Box::from_raw(in_progress);
    Box::into_raw(Box::new(in_progress.commit().into()))
}

/// Rolls back all the transacts that have been performed using this
/// in progress transaction.
///
/// Returns a [Result<()>](std::result::Result) as an [ExternResult](ExternResult).
///
/// TODO: Document the errors that can result from rollback
#[no_mangle]
pub unsafe extern "C" fn in_progress_rollback<'m>(in_progress: *mut InProgress<'m, 'm>) -> *mut ExternResult {
    let in_progress = Box::from_raw(in_progress);
    Box::into_raw(Box::new(in_progress.rollback().into()))
}

/// Creates a builder using the in progress transaction to allow for programmatic
/// assertion of values.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `in_progress_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder<'m>(in_progress: *mut InProgress<'m, 'm>) -> *mut InProgressBuilder {
    let in_progress = Box::from_raw(in_progress);
    Box::into_raw(Box::new(in_progress.builder()))
}

/// Creates a builder for an entity with `tempid` using the in progress transaction to
/// allow for programmatic assertion of values for that entity.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `entity_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn in_progress_entity_builder_from_temp_id<'m>(in_progress: *mut InProgress<'m, 'm>, temp_id: *const c_char) -> *mut EntityBuilder<InProgressBuilder> {
    let in_progress = Box::from_raw(in_progress);
    let temp_id = c_char_to_string(temp_id);
    Box::into_raw(Box::new(in_progress.builder().describe_tempid(&temp_id)))
}

/// Creates a builder for an entity with `entid` using the in progress transaction to
/// allow for programmatic assertion of values for that entity.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `entity_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn in_progress_entity_builder_from_entid<'m>(in_progress: *mut InProgress<'m, 'm>, entid: c_longlong) -> *mut EntityBuilder<InProgressBuilder> {
    let in_progress = Box::from_raw(in_progress);
    Box::into_raw(Box::new(in_progress.builder().describe(&KnownEntid(entid))))
}

/// Starts a new transaction and creates a builder using the transaction
/// to allow for programmatic assertion of values.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `in_progress_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn store_in_progress_builder(store: *mut Store) -> *mut ExternResult {
    let store = &mut *store;
    let result = store.begin_transaction().and_then(|in_progress| {
        Ok(in_progress.builder())
    });
    Box::into_raw(Box::new(result.into()))
}

/// Starts a new transaction and creates a builder for an entity with `tempid`
/// using the transaction to allow for programmatic assertion of values for that entity.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `entity_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn store_entity_builder_from_temp_id(store: *mut Store, temp_id: *const c_char) -> *mut ExternResult {
    let store = &mut *store;
    let temp_id = c_char_to_string(temp_id);
    let result = store.begin_transaction().and_then(|in_progress| {
        Ok(in_progress.builder().describe_tempid(&temp_id))
    });
    Box::into_raw(Box::new(result.into()))
}

/// Starts a new transaction and creates a builder for an entity with `entid`
/// using the transaction to allow for programmatic assertion of values for that entity.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `entity_builder_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn store_entity_builder_from_entid(store: *mut Store, entid: c_longlong) -> *mut ExternResult {
    let store = &mut *store;
    let result = store.begin_transaction().and_then(|in_progress| {
        Ok(in_progress.builder().describe(&KnownEntid(entid)))
    });
    Box::into_raw(Box::new(result.into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/string`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_string<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = c_char_to_string(value).into();
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/long`.
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_long<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Long(value);
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If `value` is not present as an Entid in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/ref`.
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_ref<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Ref(value);
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If `value` is not present as an attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/keyword`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_keyword<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = kw_from_string(c_char_to_string(value)).into();
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/boolean`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_boolean<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: bool) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/double`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_double<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: f64) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/instant`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_timestamp<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::instant(value);
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/uuid`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_add_uuid<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *mut [u8; 16]) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value = &*value;
    let value = Uuid::from_bytes(value).expect("valid uuid");
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/string`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_string<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = c_char_to_string(value).into();
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/long`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_long<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Long(value);
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/ref`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_ref<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Ref(value);
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}


/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/keyword`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_keyword<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = kw_from_string(c_char_to_string(value)).into();
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/boolean`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_boolean<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: bool) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/double`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_double<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: f64) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/instant`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_timestamp<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::instant(value);
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/uuid`.
///
// TODO don't panic if the UUID is not valid - return result instead.
//
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_retract_uuid<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>, entid: c_longlong, kw: *const c_char, value: *mut [u8; 16]) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value = &*value;
    let value = Uuid::from_bytes(value).expect("valid uuid");
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(KnownEntid(entid), &kw, value).into()))
}

/// Transacts and commits all the assertions and retractions that have been performed
/// using this builder.
///
/// This consumes the builder and the enclosed [InProgress](mentat::InProgress) transaction.
///
/// Returns a [Result<()>(std::result::Result) as an [ExternResult](ExternResult).
///
// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_commit<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>) -> *mut ExternResult {
    let builder = Box::from_raw(builder);
    Box::into_raw(Box::new(builder.commit().into()))
}

/// Transacts all the assertions and retractions that have been performed
/// using this builder.
///
/// This consumes the builder and returns the enclosed [InProgress](mentat::InProgress) transaction
/// inside the [InProgressTransactResult](mentat::InProgressTransactResult) alongside the [TxReport](mentat::TxReport) generated
/// by the transact.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// The destructors `in_progress_destroy` and `tx_report_destroy` arew provided for
/// releasing the memory for these pointer types.
///
// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn in_progress_builder_transact<'a, 'c>(builder: *mut InProgressBuilder<'a, 'c>) -> *mut InProgressTransactResult<'a, 'c> {
    let builder = Box::from_raw(builder);
    let (in_progress, tx_report) = builder.transact();
    let result = InProgressTransactResult { in_progress: Box::into_raw(Box::new(in_progress)), result: Box::into_raw(Box::new(tx_report.into())) };
    Box::into_raw(Box::new(result))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/string`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_string<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = c_char_to_string(value).into();
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/long`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_long<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Long(value);
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/ref`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_ref<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Ref(value);
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/keyword`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_keyword<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = kw_from_string(c_char_to_string(value)).into();
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/boolean`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_boolean<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: bool) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/double`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_double<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: f64) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/instant`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_timestamp<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::instant(value);
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to assert `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/uuid`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_add_uuid<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *mut [u8; 16]) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value = &*value;
    let value = Uuid::from_bytes(value).expect("valid uuid");
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.add_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/string`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_string<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = c_char_to_string(value).into();
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/long`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_long<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Long(value);
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/ref`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_ref<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::Ref(value);
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/keyword`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_keyword<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *const c_char) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = kw_from_string(c_char_to_string(value)).into();
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/boolean`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_boolean<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: bool) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/double`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_double<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: f64) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/instant`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_timestamp<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: c_longlong) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value: TypedValue = TypedValue::instant(value);
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Uses `builder` to retract `value` for `kw` on entity `entid`.
///
/// # Errors
///
/// If `entid` is not present in the store.
/// If `kw` is not a valid attribute in the store.
/// If the `:db/type` of the attribute described by `kw` is not `:db.type/uuid`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
// TODO don't panic if the UUID is not valid - return result instead.
#[no_mangle]
pub unsafe extern "C" fn entity_builder_retract_uuid<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>, kw: *const c_char, value: *mut [u8; 16]) -> *mut ExternResult {
    let builder = &mut *builder;
    let kw = kw_from_string(c_char_to_string(kw));
    let value = &*value;
    let value = Uuid::from_bytes(value).expect("valid uuid");
    let value: TypedValue = value.into();
    Box::into_raw(Box::new(builder.retract_kw(&kw, value).into()))
}

/// Transacts all the assertions and retractions that have been performed
/// using this builder.
///
/// This consumes the builder and returns the enclosed [InProgress](mentat::InProgress) transaction
/// inside the [InProgressTransactResult][::InProgressTransactResult] alongside the [TxReport](mentat::TxReport) generated
/// by the transact.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// The destructors `in_progress_destroy` and `tx_report_destroy` are provided for
/// releasing the memory for these pointer types.
///
/// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn entity_builder_transact<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>) -> *mut InProgressTransactResult<'a, 'c> {
    let builder = Box::from_raw(builder);
    let (in_progress, tx_report) = builder.transact();
    let result = InProgressTransactResult { in_progress: Box::into_raw(Box::new(in_progress)), result: Box::into_raw(Box::new(tx_report.into())) };
    Box::into_raw(Box::new(result))
}

/// Transacts and commits all the assertions and retractions that have been performed
/// using this builder.
///
/// This consumes the builder and the enclosed [InProgress](mentat::InProgress) transaction.
///
/// Returns a [Result](std::result::Result) as an [ExternResult](::ExternResult).
///
/// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn entity_builder_commit<'a, 'c>(builder: *mut EntityBuilder<InProgressBuilder<'a, 'c>>) -> *mut ExternResult {
    let builder = Box::from_raw(builder);
    Box::into_raw(Box::new(builder.commit().into()))
}

/// Performs a single transaction against the store.
///
/// Returns a [TxReport](mentat::TxReport) as an [ExternResult](::ExternResult).
/// TODO: Document the errors that can result from transact
#[no_mangle]
pub unsafe extern "C" fn store_transact(store: *mut Store, transaction: *const c_char) -> *mut ExternResult {
    let store = &mut *store;
    let transaction = c_char_to_string(transaction);
    let result = store.begin_transaction().and_then(|mut in_progress| {
        in_progress.transact(transaction).and_then(|tx_report| {
            in_progress.commit()
                       .map(|_| tx_report)
        })
    });
    Box::into_raw(Box::new(result.into()))
}

/// Fetches the `tx_id` for the given [TxReport](mentat::TxReport)`.
#[no_mangle]
pub unsafe extern "C" fn tx_report_get_entid(tx_report: *mut TxReport) -> c_longlong {
    let tx_report = &*tx_report;
    tx_report.tx_id as c_longlong
}

/// Fetches the `tx_instant` for the given [TxReport](mentat::TxReport).
#[no_mangle]
pub unsafe extern "C" fn tx_report_get_tx_instant(tx_report: *mut TxReport) -> c_longlong {
    let tx_report = &*tx_report;
    tx_report.tx_instant.timestamp() as c_longlong
}

/// Fetches the [Entid](mentat::Entid) assigned to the `tempid` during the transaction represented
/// by the given [TxReport](mentat::TxReport).
#[no_mangle]
pub unsafe extern "C" fn tx_report_entity_for_temp_id(tx_report: *mut TxReport, tempid: *const c_char) -> *mut c_longlong {
    let tx_report = &*tx_report;
    let key = c_char_to_string(tempid);
    if let Some(entid) = tx_report.tempids.get(key) {
        Box::into_raw(Box::new(entid.clone() as c_longlong))
    } else {
        std::ptr::null_mut()
    }
}

/// Adds an attribute to the cache.
/// `store_cache_attribute_forward` caches values for an attribute keyed by entity
/// (i.e. find values and entities that have this attribute, or find values of attribute for an entity)
#[no_mangle]
pub extern "C" fn store_cache_attribute_forward(store: *mut Store, attribute: *const c_char) -> *mut ExternResult {
    let store = unsafe { &mut *store };
    let kw = kw_from_string(c_char_to_string(attribute));
    Box::into_raw(Box::new(store.cache(&kw, CacheDirection::Forward).into()))
}

/// Adds an attribute to the cache.
/// `store_cache_attribute_reverse` caches entities for an attribute keyed by value.
/// (i.e. find entities that have a particular value for an attribute).
#[no_mangle]
pub extern "C" fn store_cache_attribute_reverse(store: *mut Store, attribute: *const c_char) -> *mut ExternResult {
    let store = unsafe { &mut *store };
    let kw = kw_from_string(c_char_to_string(attribute));
    Box::into_raw(Box::new(store.cache(&kw, CacheDirection::Reverse).into()))
}

/// Adds an attribute to the cache.
/// `store_cache_attribute_bi_directional` caches entity in both available directions, forward and reverse.
///
/// `Forward` caches values for an attribute keyed by entity
/// (i.e. find values and entities that have this attribute, or find values of attribute for an entity)
///
/// `Reverse` caches entities for an attribute keyed by value.
/// (i.e. find entities that have a particular value for an attribute).
#[no_mangle]
pub extern "C" fn store_cache_attribute_bi_directional(store: *mut Store, attribute: *const c_char) -> *mut ExternResult {
    let store = unsafe { &mut *store };
    let kw = kw_from_string(c_char_to_string(attribute));
    Box::into_raw(Box::new(store.cache(&kw, CacheDirection::Both).into()))
}

/// Creates a [QueryBuilder](mentat::QueryBuilder) from the given store to execute the provided query.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `query_builder_destroy` is provided for releasing the memory for this
/// pointer type.
///
/// TODO: Update QueryBuilder so it only takes a [Store](mentat::Store)  pointer on execution
#[no_mangle]
pub unsafe extern "C" fn store_query<'a>(store: *mut Store, query: *const c_char) -> *mut QueryBuilder<'a> {
    let query = c_char_to_string(query);
    let store = &mut *store;
    let query_builder = QueryBuilder::new(store, query);
    Box::into_raw(Box::new(query_builder))
}

/// Binds a [TypedValue::Long](mentat::TypedValue::Long) to a [Variable](mentat::Variable) with the given name.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_long(query_builder: *mut QueryBuilder, var: *const c_char, value: c_longlong) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
   query_builder.bind_long(&var, value);
}

/// Binds a [TypedValue::Ref](mentat::TypedValue::Ref) to a [Variable](mentat::Variable) with the given name.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_ref(query_builder: *mut QueryBuilder, var: *const c_char, value: c_longlong) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
    query_builder.bind_ref(&var, value);
}

/// Binds a [TypedValue::Ref](mentat::TypedValue::Ref) to a [Variable](mentat::Variable) with the given name. Takes a keyword as a c string in the format
/// `:namespace/name` and converts it into an [NamespacedKeyworf](mentat::NamespacedKeyword).
///
/// # Panics
///
/// If the provided keyword does not map to a valid keyword in the schema.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_ref_kw(query_builder: *mut QueryBuilder, var: *const c_char, value: *const c_char) {
    let var = c_char_to_string(var);
    let kw = kw_from_string(c_char_to_string(value));
    let query_builder = &mut *query_builder;
    if let Some(err) = query_builder.bind_ref_from_kw(&var, kw).err() {
        panic!(err);
    }
}

/// Binds a [TypedValue::Ref](mentat::TypedValue::Ref) to a [Variable](mentat::Variable) with the given name. Takes a keyword as a c string in the format
/// `:namespace/name` and converts it into an [NamespacedKeyworf](mentat::NamespacedKeyword).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_kw(query_builder: *mut QueryBuilder, var: *const c_char, value: *const c_char) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
    let kw = kw_from_string(c_char_to_string(value));
    query_builder.bind_value(&var, kw);
}

/// Binds a [TypedValue::Boolean](mentat::TypedValue::Boolean) to a [Variable](mentat::Variable) with the given name.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_boolean(query_builder: *mut QueryBuilder, var: *const c_char, value: bool) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
    query_builder.bind_value(&var, value);
}

/// Binds a [TypedValue::Double](mentat::TypedValue::Double) to a [Variable](mentat::Variable) with the given name.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_double(query_builder: *mut QueryBuilder, var: *const c_char, value: f64) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
    query_builder.bind_value(&var, value);
}

/// Binds a [TypedValue::Instant](mentat::TypedValue::Instant) to a [Variable](mentat::Variable) with the given name.
/// Takes a timestamp in microseconds.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_timestamp(query_builder: *mut QueryBuilder, var: *const c_char, value: c_longlong) {
    let var = c_char_to_string(var);
    let query_builder = &mut *query_builder;
    query_builder.bind_instant(&var, value);
}

/// Binds a [TypedValue::String](mentat::TypedValue::String) to a [Variable](mentat::Variable) with the given name.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_string(query_builder: *mut QueryBuilder, var: *const c_char, value: *const c_char) {
    let var = c_char_to_string(var);
    let value = c_char_to_string(value);
    let query_builder = &mut *query_builder;
    query_builder.bind_value(&var, value);
}

/// Binds a [TypedValue::Uuid](mentat::TypedValue::Uuid) to a [Variable](mentat::Variable) with the given name.
/// Takes a `UUID` as a byte slice of length 16. This maps directly to the `uuid_t` C type.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn query_builder_bind_uuid(query_builder: *mut QueryBuilder, var: *const c_char, value: *mut [u8; 16]) {
    let var = c_char_to_string(var);
    let value = &*value;
    let value = Uuid::from_bytes(value).expect("valid uuid");
    let query_builder = &mut *query_builder;
    query_builder.bind_value(&var, value);
}

/// Executes a query and returns the results as a [Scalar](mentat::QueryResults::Scalar).
///
/// # Panics
///
/// If the find set of the query executed is not structured `[:find ?foo . :where ...]`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn query_builder_execute_scalar(query_builder: *mut QueryBuilder) -> *mut ExternResult {
    let query_builder = &mut *query_builder;
    let results = query_builder.execute_scalar();
    let extern_result = match results {
        Ok(Some(v)) => ExternResult { err: std::ptr::null(), ok: Box::into_raw(Box::new(v)) as *const _ as *const c_void, },
        Ok(None) => ExternResult { err: std::ptr::null(), ok: std::ptr::null(), },
        Err(e) => ExternResult { err: string_to_c_char(e.to_string()), ok: std::ptr::null(), }
    };
    Box::into_raw(Box::new(extern_result))
}

/// Executes a query and returns the results as a [Coll](mentat::QueryResults::Coll).
///
/// # Panics
///
/// If the find set of the query executed is not structured `[:find [?foo ...] :where ...]`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn query_builder_execute_coll(query_builder: *mut QueryBuilder) -> *mut ExternResult {
    let query_builder = &mut *query_builder;
    let results = query_builder.execute_coll();
    Box::into_raw(Box::new(results.into()))
}

/// Executes a query and returns the results as a [Tuple](mentat::QueryResults::Tuple).
///
/// # Panics
///
/// If the find set of the query executed is not structured `[:find [?foo ?bar] :where ...]`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn query_builder_execute_tuple(query_builder: *mut QueryBuilder) -> *mut ExternResult {
    let query_builder = &mut *query_builder;
    let results = query_builder.execute_tuple();
    let extern_result = match results {
        Ok(Some(v)) => ExternResult { err: std::ptr::null(), ok: Box::into_raw(Box::new(v)) as *const _ as *const c_void, },
        Ok(None) => ExternResult { err: std::ptr::null(), ok: std::ptr::null(), },
        Err(e) => ExternResult { err: string_to_c_char(e.to_string()), ok: std::ptr::null(), }
    };
    Box::into_raw(Box::new(extern_result))
}

/// Executes a query and returns the results as a [Rel](mentat::QueryResults::Rel).
///
/// # Panics
///
/// If the find set of the query executed is not structured `[:find ?foo ?bar :where ...]`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn query_builder_execute(query_builder: *mut QueryBuilder) -> *mut ExternResult {
    let query_builder = &mut *query_builder;
    let results = query_builder.execute_rel();
    Box::into_raw(Box::new(results.into()))
}

fn unwrap_conversion<T>(value: Option<T>, expected_type: ValueType) -> T {
    match value {
        Some(v) => v,
        None => panic!("Typed value cannot be coerced into a {}", expected_type)
    }
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a C `long`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Long).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_long(typed_value: *mut Binding) -> c_longlong {
    let typed_value = Box::from_raw(typed_value);
    unwrap_conversion(typed_value.into_long(), ValueType::Long)
}

/// Consumes a [Binding](mentat::Binding) and returns the value as an [Entid](mentat::Entid).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Ref](mentat::ValueType::Ref).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_entid(typed_value: *mut Binding) -> Entid {
    let typed_value = Box::from_raw(typed_value);
    println!("typed value as entid {:?}", typed_value);
    unwrap_conversion(typed_value.into_entid(), ValueType::Ref)
}

/// Consumes a [Binding](mentat::Binding) and returns the value as an keyword C `String`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Ref](mentat::ValueType::Ref).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_kw(typed_value: *mut Binding) -> *const c_char {
    let typed_value = Box::from_raw(typed_value);
    unwrap_conversion(typed_value.into_kw_c_string(), ValueType::Keyword) as *const c_char
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a boolean represented as an `i32`.
/// If the value of the boolean is `true` the value returned is 1.
/// If the value of the boolean is `false` the value returned is 0.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Boolean).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_boolean(typed_value: *mut Binding) -> i32 {
    let typed_value = Box::from_raw(typed_value);
    if unwrap_conversion(typed_value.into_boolean(), ValueType::Boolean) { 1 } else { 0 }
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a `f64`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Double).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_double(typed_value: *mut Binding) -> f64 {
    let typed_value = Box::from_raw(typed_value);
    unwrap_conversion(typed_value.into_double(), ValueType::Double)
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a microsecond timestamp.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Instant).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_timestamp(typed_value: *mut Binding) -> c_longlong {
    let typed_value = Box::from_raw(typed_value);
    unwrap_conversion(typed_value.into_timestamp(), ValueType::Instant)
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a C `String`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::String).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_string(typed_value: *mut Binding) -> *const c_char {
    let typed_value = Box::from_raw(typed_value);
    unwrap_conversion(typed_value.into_c_string(), ValueType::String) as *const c_char
}

/// Consumes a [Binding](mentat::Binding) and returns the value as a UUID byte slice of length 16.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Uuid).
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn typed_value_into_uuid(typed_value: *mut Binding) -> *mut [u8; 16] {
    let typed_value = Box::from_raw(typed_value);
    let value = unwrap_conversion(typed_value.into_uuid(), ValueType::Uuid);
    Box::into_raw(Box::new(*value.as_bytes()))
}

/// Returns the [ValueType](mentat::ValueType) of this [Binding](mentat::Binding).
#[no_mangle]
pub unsafe extern "C" fn typed_value_value_type(typed_value: *mut Binding) -> ValueType {
    let typed_value = &*typed_value;
    typed_value.value_type().unwrap_or_else(|| panic!("Binding is not Scalar and has no ValueType"))
}

/// Returns the value at the provided `index` as a `Vec<ValueType>`.
/// If there is no value present at the `index`, a null pointer is returned.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_result_set_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn row_at_index(rows: *mut RelResult<Binding>, index: c_int) -> *mut Vec<Binding> {
    let result = &*rows;
    result.row(index as usize).map_or_else(std::ptr::null_mut, |v| Box::into_raw(Box::new(v.to_vec())))
}

/// Consumes the `RelResult<Binding>` and returns an iterator over the values.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_result_set_iter_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn typed_value_result_set_into_iter(rows: *mut RelResult<Binding>) -> *mut BindingListIterator {
    let result = &*rows;
    let rows = result.rows();
    Box::into_raw(Box::new(rows))
}

/// Returns the next value in the `iter` as a `Vec<ValueType>`.
/// If there is no value next value, a null pointer is returned.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_list_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn typed_value_result_set_iter_next(iter: *mut BindingListIterator) -> *mut Vec<Binding> {
    let iter = &mut *iter;
    iter.next().map_or(std::ptr::null_mut(), |v| Box::into_raw(Box::new(v.to_vec())))
}

/// Consumes the `Vec<Binding>` and returns an iterator over the values.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_list_iter_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn typed_value_list_into_iter(values: *mut Vec<Binding>) -> *mut BindingIterator {
    let result = Box::from_raw(values);
    Box::into_raw(Box::new(result.into_iter()))
}

/// Returns the next value in the `iter` as a [Binding](mentat::Binding).
/// If there is no value next value, a null pointer is returned.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn typed_value_list_iter_next(iter: *mut BindingIterator) -> *mut Binding {
    let iter = &mut *iter;
    iter.next().map_or(std::ptr::null_mut(), |v| Box::into_raw(Box::new(v)))
}

/// Returns the value at the provided `index` as a [Binding](mentat::Binding).
/// If there is no value present at the `index`, a null pointer is returned.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn value_at_index(values: *mut Vec<Binding>, index: c_int) -> *const Binding {
    let result = &*values;
    result.get(index as usize).expect("No value at index") as *const Binding
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a `long`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not `ValueType::Long`.
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_long(values: *mut Vec<Binding>, index: c_int) -> c_longlong {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_long(), ValueType::Long)
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as an [Entid](mentat::Entid).
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not `ValueType::Ref`.
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_entid(values: *mut Vec<Binding>, index: c_int) -> Entid {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_entid(), ValueType::Ref)
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a keyword C `String`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Ref](mentat::ValueType::Ref).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_kw(values: *mut Vec<Binding>, index: c_int) -> *const c_char {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_kw_c_string(), ValueType::Keyword) as *const c_char
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a boolean represented by a `i32`.
/// If the value of the `boolean` is `true` then the value returned is 1.
/// If the value of the `boolean` is `false` then the value returned is 0.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Long](mentat::ValueType::Long).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_boolean(values: *mut Vec<Binding>, index: c_int) -> i32 {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    if unwrap_conversion(value.clone().into_boolean(), ValueType::Boolean) { 1 } else { 0 }
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as an `f64`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Double](mentat::ValueType::Double).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_double(values: *mut Vec<Binding>, index: c_int) -> f64 {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_double(), ValueType::Double)
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a microsecond timestamp.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Instant](mentat::ValueType::Instant).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_timestamp(values: *mut Vec<Binding>, index: c_int) -> c_longlong {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_timestamp(), ValueType::Instant)
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a C `String`.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::String](mentat::ValueType::String).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_string(values: *mut Vec<Binding>, index: c_int) -> *const c_char {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    unwrap_conversion(value.clone().into_c_string(), ValueType::String) as *const c_char
}

/// Returns the value of the [Binding](mentat::Binding) at `index` as a UUID byte slice of length 16.
///
/// # Panics
///
/// If the [ValueType](mentat::ValueType) of the [Binding](mentat::Binding) is not [ValueType::Uuid](mentat::ValueType::Uuid).
/// If there is no value at `index`.
///
// TODO Generalise with macro https://github.com/mozilla/mentat/issues/703
#[no_mangle]
pub unsafe extern "C" fn value_at_index_into_uuid(values: *mut Vec<Binding>, index: c_int) -> *mut [u8; 16] {
    let result = &*values;
    let value = result.get(index as usize).expect("No value at index");
    let uuid = unwrap_conversion(value.clone().into_uuid(), ValueType::Uuid);
    Box::into_raw(Box::new(*uuid.as_bytes()))
}

/// Returns an [ExternResult](ExternResult) containing the [Binding](mentat::Binding) associated with the `attribute` as `:namespace/name`
/// for the given `entid`.
/// If there is a value for that `attribute` on the entity with id `entid` then the value is returned in `ok`.
/// If there no value for that `attribute` on the entity with id `entid` but the attribute is value,
/// then a null pointer is returned in `ok`.
/// If there is no [Attribute](mentat::Attribute) in the [Schema](mentat::Schema) for the given `attribute` then an error is returned in `err`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `destroy` is provided for releasing the memory for this
/// pointer type.
///
/// TODO: list the types of error that can be caused by this function
#[no_mangle]
pub unsafe extern "C" fn store_value_for_attribute(store: *mut Store, entid: c_longlong, attribute: *const c_char) -> *mut ExternResult {
    let store = &*store;
    let kw = kw_from_string(c_char_to_string(attribute));
    let value = match store.lookup_value_for_attribute(entid, &kw) {
        Ok(Some(v)) => {
            let value: Binding = v.into();
            ExternResult { ok: Box::into_raw(Box::new(value)) as *const _ as *const c_void, err: std::ptr::null() }
        },
        Ok(None) => ExternResult { ok: std::ptr::null(), err: std::ptr::null() },
        Err(e) => ExternResult { ok: std::ptr::null(), err: string_to_c_char(e.to_string()) },
    };
    Box::into_raw(Box::new(value))
}

/// Registers a [TxObserver](mentat::TxObserver) with the `key` to observe changes to `attributes`
/// on this `store`.
/// Calls `callback` is a relevant transaction occurs.
///
/// # Panics
///
/// If there is no [Attribute](mentat::Attribute)  in the [Schema](mentat::Schema)  for a given `attribute`.
///
#[no_mangle]
pub unsafe extern "C" fn store_register_observer(store: *mut Store,
                                                   key: *const c_char,
                                            attributes: *const Entid,
                                        attributes_len: usize,
                                              callback: extern fn(key: *const c_char, reports: &TxChangeList)) {
    let store = &mut *store;
    let mut attribute_set = BTreeSet::new();
    let slice = slice::from_raw_parts(attributes, attributes_len);
    attribute_set.extend(slice.iter());
    let key = c_char_to_string(key);
    let tx_observer = Arc::new(TxObserver::new(attribute_set, move |obs_key, batch| {
        let extern_reports: Vec<TransactionChange> = batch.into_iter().map(|(tx_id, changes)| {
            let changes: Vec<Entid> = changes.into_iter().map(|i|*i).collect();
            let len = changes.len();
            TransactionChange {
                txid: *tx_id,
                changes: changes.into_boxed_slice(),
                changes_len: len,
            }
        }).collect();
        let len = extern_reports.len();
        let reports = TxChangeList {
            reports: extern_reports.into_boxed_slice(),
            len: len,
        };
        callback(string_to_c_char(obs_key), &reports);
    }));
    store.register_observer(key.to_string(), tx_observer);
}

/// Unregisters a [TxObserver](mentat::TxObserver)  with the `key` to observe changes on this `store`.
#[no_mangle]
pub unsafe extern "C" fn store_unregister_observer(store: *mut Store, key: *const c_char) {
    let store = &mut *store;
    let key = c_char_to_string(key).to_string();
    store.unregister_observer(&key);
}

/// Returns the [Entid](mentat::Entid)  associated with the `attr` as `:namespace/name`.
///
/// # Panics
///
/// If there is no [Attribute](mentat::Attribute)  in the [Schema](mentat::Schema)  for `attr`.
#[no_mangle]
pub unsafe extern "C" fn store_entid_for_attribute(store: *mut Store, attr: *const c_char) -> Entid {
    let store = &mut *store;
    let keyword_string = c_char_to_string(attr);
    let kw = kw_from_string(keyword_string);
    let conn = store.conn();
    let current_schema = conn.current_schema();
    current_schema.get_entid(&kw).expect("Unable to find entid for invalid attribute").into()
}

/// Returns the value at the provided `index` as a [TransactionChange](TransactionChange) .
///
/// # Panics
///
/// If there is no value present at the `index`.
///
/// # Safety
///
/// Callers are responsible for managing the memory for the return value.
/// A destructor `typed_value_destroy` is provided for releasing the memory for this
/// pointer type.
#[no_mangle]
pub unsafe extern "C" fn tx_change_list_entry_at(tx_report_list: *mut TxChangeList, index: c_int) -> *const TransactionChange {
    let tx_report_list = &*tx_report_list;
    let index = index as usize;
    let report = Box::new(tx_report_list.reports[index].clone());
    Box::into_raw(report)
}

/// Returns the value at the provided `index` as a [Entid](mentat::Entid) .
///
/// # Panics
///
/// If there is no value present at the `index`.
#[no_mangle]
pub unsafe extern "C" fn changelist_entry_at(tx_report: *mut TransactionChange, index: c_int) -> Entid {
    let tx_report = &*tx_report;
    let index = index as usize;
    tx_report.changes[index].clone()
}

/// Creates a function with a given `$name` that releases the memory for a type `$t`.
macro_rules! define_destructor (
    ($name:ident, $t:ty) => (
        #[no_mangle]
        pub unsafe extern "C" fn $name(obj: *mut $t) {
            let _ = Box::from_raw(obj);
        }
    )
);

/// Creates a function with a given `$name` that releases the memory
/// for a type `$t` with lifetimes <'a, 'c>.
/// TODO: Move to using `macro_rules` lifetime specifier when it lands in stable
/// This will enable us to specialise `define_destructor` and use repetitions
/// to allow more generic lifetime handling instead of having two functions.
/// https://github.com/rust-lang/rust/issues/34303
/// https://github.com/mozilla/mentat/issues/702
macro_rules! define_destructor_with_lifetimes (
    ($name:ident, $t:ty) => (
        #[no_mangle]
        pub unsafe extern "C" fn $name<'a, 'c>(obj: *mut $t) {
            let _ = Box::from_raw(obj);
        }
    )
);

/// destroy function for releasing the memory for `repr(C)` structs.
define_destructor!(destroy, c_void);

/// Destructor for releasing the memory of [InProgressBuilder](mentat::InProgressBuilder).
define_destructor_with_lifetimes!(in_progress_builder_destroy, InProgressBuilder<'a, 'c>);

/// Destructor for releasing the memory of [EntityBuilder](mentat::EntityBuilder).
define_destructor_with_lifetimes!(entity_builder_destroy, EntityBuilder<InProgressBuilder<'a, 'c>>);

/// Destructor for releasing the memory of [QueryBuilder](mentat::QueryBuilder) .
define_destructor!(query_builder_destroy, QueryBuilder);

/// Destructor for releasing the memory of [Store](mentat::Store) .
define_destructor!(store_destroy, Store);

/// Destructor for releasing the memory of [TxReport](mentat::TxReport) .
define_destructor!(tx_report_destroy, TxReport);

/// Destructor for releasing the memory of [Binding](mentat::Binding).
define_destructor!(typed_value_destroy, Binding);

/// Destructor for releasing the memory of [Vec<Binding>][mentat::Binding].
define_destructor!(typed_value_list_destroy, Vec<Binding>);

/// Destructor for releasing the memory of [BindingIterator](BindingIterator) .
define_destructor!(typed_value_list_iter_destroy, BindingIterator);

/// Destructor for releasing the memory of [RelResult<Binding>](mentat::RelResult).
define_destructor!(typed_value_result_set_destroy, RelResult<Binding>);

/// Destructor for releasing the memory of [BindingListIterator](::BindingListIterator).
define_destructor!(typed_value_result_set_iter_destroy, BindingListIterator);

/// Destructor for releasing the memory of [InProgress](mentat::InProgress).
define_destructor!(in_progress_destroy, InProgress);
