pub mod error;
pub mod schema;
pub mod storage;
pub mod query;
pub mod compaction;
pub mod wal;
pub mod database;
pub mod persistence;

pub use error::*;
pub use schema::*;
pub use storage::*;
pub use query::*;
pub use compaction::*;
pub use wal::*;
pub use database::*;
pub use persistence::*;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
