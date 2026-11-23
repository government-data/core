//! The contract suite of the capability `lake` (decision 0078, point 6).
//! A lake is accepted when it passes:
//!
//! - [`ports`]: the two ports, each step run twice and changing nothing;
//! - [`admin_wire`]: `lake-admin/1` at the wire, every operation run twice,
//!   a caller refused by its certificate, the error shape;
//! - [`data_wire`]: S3, Iceberg REST and STS at the wire: an object put
//!   once, a session policy that refuses outside its grant, a table
//!   created, committed, listed, time-travelled and dropped, a namespace
//!   made twice.
//!
//! Against the in-memory fake both run in every build (`tests/fake.rs`,
//! the fake served in process behind the same mTLS check); against RustFS
//! in its verification (step 14), with the cases of S3, Iceberg REST and
//! STS at the wire. Every case panics with what failed, as a test does.

pub mod admin_wire;
pub mod data_wire;
pub mod ports;
