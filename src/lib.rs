pub mod error;
pub mod schema;
pub mod storage;
pub mod query;
pub mod compaction;
pub mod wal;
pub mod database;

pub use error::*;
pub use schema::*;
pub use storage::*;
pub use query::*;
pub use compaction::*;
pub use wal::*;
pub use database::*;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
