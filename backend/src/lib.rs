//! twt-cloud backend library: lib+bin split so integration tests
//! (tests/) can exercise the router, auth and route handlers directly.

pub mod api;
pub mod auth;
pub mod error;
pub mod routes;
