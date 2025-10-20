use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
    
    #[error("Parse int error: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    
    #[error("Parse float error: {0}")]
    ParseFloat(#[from] std::num::ParseFloatError),
    
    #[error("Parse bool error: {0}")]
    ParseBool(#[from] std::str::ParseBoolError),
    
    #[error("Snap error: {0}")]
    Snap(#[from] snap::Error),
    
    #[error("LZ4 error: {0}")]
    LZ4(#[from] lz4_flex::block::DecompressError),
    
    #[error("ZSTD error: {0}")]
    ZSTD(#[from] std::io::Error),
    
    #[error("Table not found: {table}")]
    TableNotFound { table: String },
    
    #[error("Keyspace not found: {keyspace}")]
    KeyspaceNotFound { keyspace: String },
    
    #[error("Invalid schema: {message}")]
    InvalidSchema { message: String },
    
    #[error("Query parsing error: {message}")]
    QueryParsingError { message: String },
    
    #[error("Invalid data type: {message}")]
    InvalidDataType { message: String },
    
    #[error("Memory table is full")]
    MemtableFull,
    
    #[error("Compaction error: {message}")]
    CompactionError { message: String },
    
    #[error("Commit log error: {message}")]
    CommitLogError { message: String },
    
    #[error("Generic error: {message}")]
    Generic { message: String },
}

pub type Result<T> = std::result::Result<T, CoreDBError>;

impl From<String> for CoreDBError {
    fn from(message: String) -> Self {
        CoreDBError::Generic { message }
    }
}

impl From<&str> for CoreDBError {
    fn from(message: &str) -> Self {
        CoreDBError::Generic { message: message.to_string() }
    }
}
