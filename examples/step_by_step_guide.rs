use coredb::database::{CoreDB, DatabaseConfig};
use coredb::schema::{CassandraDataType, ColumnDefinition, TableSchema};
use coredb::query::QueryResult;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 CoreDB Step-by-Step Guide");
    println!("===========================\n");

    // 0. Clean up previous run
    let data_dir = PathBuf::from("guide_data");
    if data_dir.exists() {
        tokio::fs::remove_dir_all(&data_dir).await?;
    }
    
    // 1. Initialize Database
    println!("1️⃣  Initializing Database...");
    let config = DatabaseConfig {
        data_directory: data_dir.clone(),
        commitlog_directory: data_dir.join("commitlog"),
        ..DatabaseConfig::default()
    };
    
    let db = CoreDB::new(config).await?;
    println!("   ✅ Database initialized at {:?}", data_dir);

    // 2. Create Keyspace (API Usage)
    println!("\n2️⃣  Creating Keyspace 'store'...");
    db.create_keyspace("store".to_string(), 1).await?;
    println!("   ✅ Keyspace 'store' created");

    // 3. Create Table (API Usage - Simple Schema)
    println!("\n3️⃣  Creating Table 'store.products'...");
    // Schema:
    // - Partition Key: id (INT)
    // - Columns: name (TEXT), price (INT)
    
    let schema = TableSchema::new(
        "products".to_string(),
        "store".to_string(),
        vec![ColumnDefinition { 
            name: "id".to_string(), 
            data_type: CassandraDataType::Int, 
            is_static: false 
        }], // Partition Key
        vec![], // No Clustering Key
        vec![
            ColumnDefinition { 
                name: "name".to_string(), 
                data_type: CassandraDataType::Text, 
                is_static: false 
            },
            ColumnDefinition { 
                name: "price".to_string(), 
                data_type: CassandraDataType::Int, 
                is_static: false 
            }
        ],
        vec![], // Static columns
    );
    
    db.create_table("store".to_string(), "products".to_string(), schema).await?;
    println!("   ✅ Table 'store.products' created");
    println!("      Partition Key: [id]");

    // 4. Insert Data (CQL Usage)
    println!("\n4️⃣  Inserting Data...");
    
    db.execute_cql("INSERT INTO store.products (id, name, price) VALUES (1, 'Laptop', 1200)").await?;
    db.execute_cql("INSERT INTO store.products (id, name, price) VALUES (2, 'Mouse', 50)").await?;
    db.execute_cql("INSERT INTO store.products (id, name, price) VALUES (3, 'Monitor', 300)").await?;
    
    println!("   ✅ Inserted 3 products");

    // 5. Select Data (CQL Usage)
    println!("\n5️⃣  Retrieving Data...");
    
    println!("   a) Point Lookup (ID = 1):");
    let result = db.execute_cql("SELECT * FROM store.products WHERE id = 1").await?;
    if let QueryResult::Rows(rows) = result {
        println!("      Found {} row(s)", rows.len());
        for row in rows {
            if let (Some(id), Some(name), Some(price)) = (
                row.columns.get("id"),
                row.columns.get("name"),
                row.columns.get("price")
            ) {
                println!("      - ID: {:?}, Name: {:?}, Price: ${:?}", id, name, price);
            }
        }
    }

    println!("\n   b) Point Lookup (ID = 2):");
    let result = db.execute_cql("SELECT * FROM store.products WHERE id = 2").await?;
    if let QueryResult::Rows(rows) = result {
        println!("      Found {} row(s)", rows.len());
        for row in rows {
            if let (Some(id), Some(name), Some(price)) = (
                row.columns.get("id"),
                row.columns.get("name"),
                row.columns.get("price")
            ) {
                println!("      - ID: {:?}, Name: {:?}, Price: ${:?}", id, name, price);
            }
        }
    }

    println!("\n   c) Point Lookup (ID = 3):");
    let result = db.execute_cql("SELECT * FROM store.products WHERE id = 3").await?;
    if let QueryResult::Rows(rows) = result {
        println!("      Found {} row(s)", rows.len());
        for row in rows {
            if let (Some(id), Some(name), Some(price)) = (
                row.columns.get("id"),
                row.columns.get("name"),
                row.columns.get("price")
            ) {
                println!("      - ID: {:?}, Name: {:?}, Price: ${:?}", id, name, price);
            }
        }
    }

    // 6. Summary
    println!("\n6️⃣  Summary:");
    println!("   ✅ Successfully demonstrated:");
    println!("      - Database initialization");
    println!("      - Keyspace creation");
    println!("      - Table creation with partition key");
    println!("      - Data insertion via CQL");
    println!("      - Data retrieval via CQL (point lookup)");
    println!("      - Commit log for durability");

    println!("\n🎉 Guide completed successfully!");
    
    // Cleanup
    tokio::fs::remove_dir_all(&data_dir).await?;
    
    Ok(())
}
