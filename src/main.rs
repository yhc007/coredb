use clap::{Parser, Subcommand};
use coredb::{CoreDB, DatabaseConfig, DatabaseStats};
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use tokio;
use tracing::{info, error, warn};

/// CoreDB - Single node Cassandra-like database
#[derive(Parser)]
#[command(name = "coredb")]
#[command(about = "A single node Cassandra-like database written in Rust")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    /// Data directory
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
    
    /// Commit log directory
    #[arg(long, default_value = "./commitlog")]
    commitlog_dir: PathBuf,
    
    /// Memtable flush threshold in MB
    #[arg(long, default_value = "64")]
    memtable_flush_threshold: u64,
    
    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the database server
    Start {
        /// Port to listen on
        #[arg(short, long, default_value = "9042")]
        port: u16,
        
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Execute a CQL query
    Query {
        /// CQL query to execute
        query: String,
    },
    /// Interactive shell
    Shell,
    /// Show database statistics
    Stats,
    /// Initialize database
    Init,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    
    // 로깅 초기화
    init_logging(&cli.log_level);
    
    // 데이터베이스 설정
    let config = DatabaseConfig {
        data_directory: cli.data_dir,
        commitlog_directory: cli.commitlog_dir,
        memtable_flush_threshold_mb: cli.memtable_flush_threshold,
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
    };
    
    match cli.command {
        Commands::Start { port, host } => {
            start_server(config, host, port).await;
        },
        Commands::Query { query } => {
            execute_query(config, query).await;
        },
        Commands::Shell => {
            start_shell(config).await;
        },
        Commands::Stats => {
            show_stats(config).await;
        },
        Commands::Init => {
            init_database(config).await;
        },
    }
}

fn init_logging(level: &str) {
    let log_level = match level.to_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    
    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();
}

async fn start_server(config: DatabaseConfig, host: String, port: u16) {
    info!("Starting CoreDB server on {}:{}", host, port);
    
    // 데이터베이스 초기화
    let db = match CoreDB::new(config).await {
        Ok(db) => {
            info!("Database initialized successfully");
            db
        },
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            process::exit(1);
        }
    };
    
    info!("CoreDB server is ready to accept connections");
    
    // 간단한 HTTP 서버 (CQL 프로토콜 대신)
    let db_arc = Arc::new(db);
    let app = axum::Router::new()
        .route("/query", axum::routing::post(query_handler))
        .route("/stats", axum::routing::get(stats_handler))
        .with_state(db_arc);
    
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", host, port)).await.unwrap();
    info!("Server listening on http://{}:{}", host, port);
    
    axum::serve(listener, app).await.unwrap();
}

async fn execute_query(config: DatabaseConfig, query: String) {
    info!("Executing query: {}", query);
    
    let db = match CoreDB::new(config).await {
        Ok(db) => db,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            process::exit(1);
        }
    };
    
    match db.execute_cql(&query).await {
        Ok(result) => {
            match result {
                coredb::query::result::QueryResult::Success => {
                    println!("Query executed successfully");
                },
                coredb::query::result::QueryResult::Rows(rows) => {
                    for row in rows {
                        println!("Row: {:?}", row.columns);
                    }
                },
                coredb::query::result::QueryResult::Schema(columns) => {
                    for column in columns {
                        println!("Column: {} ({})", column.name, column.data_type);
                    }
                },
                coredb::query::result::QueryResult::Error(message) => {
                    error!("Query error: {}", message);
                },
            }
        },
        Err(e) => {
            error!("Query execution failed: {}", e);
            process::exit(1);
        }
    }
}

async fn start_shell(config: DatabaseConfig) {
    info!("Starting CoreDB interactive shell");
    
    let db = match CoreDB::new(config).await {
        Ok(db) => db,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            process::exit(1);
        }
    };
    
    println!("CoreDB Interactive Shell");
    println!("Type 'exit' or 'quit' to exit");
    println!("Type 'help' for available commands");
    println!();
    
    loop {
        print!("coredb> ");
        use std::io::{self, Write};
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(_) => {
                let query = input.trim();
                
                if query.is_empty() {
                    continue;
                }
                
                if query == "exit" || query == "quit" {
                    println!("Goodbye!");
                    break;
                }
                
                if query == "help" {
                    print_help();
                    continue;
                }
                
                if query == "stats" {
                    let stats = db.get_stats().await;
                    print_stats(&stats);
                    continue;
                }
                
                match db.execute_cql(query).await {
                    Ok(result) => {
                        match result {
                            coredb::query::result::QueryResult::Success => {
                                println!("✓ Query executed successfully");
                            },
                            coredb::query::result::QueryResult::Rows(rows) => {
                                if rows.is_empty() {
                                    println!("No rows returned");
                                } else {
                                    for (i, row) in rows.iter().enumerate() {
                                        println!("Row {}: {:?}", i + 1, row.columns);
                                    }
                                }
                            },
                            coredb::query::result::QueryResult::Schema(columns) => {
                                println!("Schema:");
                                for column in columns {
                                    println!("  {} ({})", column.name, column.data_type);
                                }
                            },
                            coredb::query::result::QueryResult::Error(message) => {
                                println!("✗ Error: {}", message);
                            },
                        }
                    },
                    Err(e) => {
                        println!("✗ Query failed: {}", e);
                    }
                }
            },
            Err(e) => {
                error!("Failed to read input: {}", e);
                break;
            }
        }
    }
}

async fn show_stats(config: DatabaseConfig) {
    let db = match CoreDB::new(config).await {
        Ok(db) => db,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            process::exit(1);
        }
    };
    
    let stats = db.get_stats().await;
    print_stats(&stats);
}

async fn init_database(config: DatabaseConfig) {
    info!("Initializing CoreDB database");
    
    match CoreDB::new(config).await {
        Ok(db) => {
            info!("Database initialized successfully");
            
            // 샘플 데이터 생성
            let sample_queries = vec![
                "CREATE KEYSPACE demo WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}",
                "CREATE TABLE demo.users (id INT PRIMARY KEY, name TEXT, email TEXT, age INT)",
                "INSERT INTO demo.users (id, name, email, age) VALUES (1, 'John Doe', 'john@example.com', 30)",
                "INSERT INTO demo.users (id, name, email, age) VALUES (2, 'Jane Smith', 'jane@example.com', 25)",
                "INSERT INTO demo.users (id, name, email, age) VALUES (3, 'Bob Johnson', 'bob@example.com', 35)",
            ];
            
            for query in sample_queries {
                match db.execute_cql(query).await {
                    Ok(_) => info!("Executed: {}", query),
                    Err(e) => warn!("Failed to execute {}: {}", query, e),
                }
            }
            
            let stats = db.get_stats().await;
            info!("Database initialized with {} keyspaces, {} tables", 
                  stats.keyspace_count, stats.table_count);
        },
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            process::exit(1);
        }
    }
}

fn print_help() {
    println!("Available commands:");
    println!("  CREATE KEYSPACE <name> WITH REPLICATION = {{'class': 'SimpleStrategy', 'replication_factor': 1}}");
    println!("  CREATE TABLE <keyspace>.<table> (<columns>)");
    println!("  INSERT INTO <keyspace>.<table> (<columns>) VALUES (<values>)");
    println!("  SELECT <columns> FROM <keyspace>.<table> [WHERE <condition>] [LIMIT <n>]");
    println!("  DROP TABLE <keyspace>.<table>");
    println!("  DROP KEYSPACE <name>");
    println!("  stats  - Show database statistics");
    println!("  help   - Show this help message");
    println!("  exit   - Exit the shell");
}

fn print_stats(stats: &DatabaseStats) {
    println!("Database Statistics:");
    println!("  Keyspaces: {}", stats.keyspace_count);
    println!("  Tables: {}", stats.table_count);
    println!("  Memtables: {}", stats.memtable_count);
    println!("  SSTables: {}", stats.sstable_count);
    println!("  Total Size: {:.2} MB", stats.total_size_bytes as f64 / 1024.0 / 1024.0);
}

// HTTP 핸들러들
async fn query_handler(
    axum::extract::State(db): axum::extract::State<std::sync::Arc<CoreDB>>,
    axum::extract::Json(payload): axum::extract::Json<serde_json::Value>,
) -> axum::response::Json<serde_json::Value> {
    let query = payload.get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    
    match db.execute_cql(query).await {
        Ok(result) => {
            let response = match result {
                coredb::query::result::QueryResult::Success => {
                    serde_json::json!({"status": "success", "message": "Query executed successfully"})
                },
                coredb::query::result::QueryResult::Rows(rows) => {
                    serde_json::json!({"status": "success", "data": rows})
                },
                coredb::query::result::QueryResult::Schema(columns) => {
                    serde_json::json!({"status": "success", "schema": columns})
                },
                coredb::query::result::QueryResult::Error(message) => {
                    serde_json::json!({"status": "error", "message": message})
                },
            };
            axum::response::Json(response)
        },
        Err(e) => {
            axum::response::Json(serde_json::json!({
                "status": "error",
                "message": e.to_string()
            }))
        }
    }
}

async fn stats_handler(
    axum::extract::State(db): axum::extract::State<std::sync::Arc<CoreDB>>,
) -> axum::response::Json<serde_json::Value> {
    let stats = db.get_stats().await;
    axum::response::Json(serde_json::json!({
        "keyspaces": stats.keyspace_count,
        "tables": stats.table_count,
        "memtables": stats.memtable_count,
        "sstables": stats.sstable_count,
        "total_size_bytes": stats.total_size_bytes
    }))
}

// Cargo.toml에 필요한 의존성 추가
// axum = "0.7"
// tower = "0.4"
// tower-http = "0.5"
