use coredb::database::{CoreDB, DatabaseConfig};

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config = DatabaseConfig::default();
    let db = CoreDB::new(config).await?;
    
    println!("=== Testing Authentication ===\n");
    
    // CREATE USER
    let result = db.execute_cql("CREATE USER admin WITH PASSWORD 'secret123' SUPERUSER").await?;
    println!("CREATE USER admin: {:?}", result);
    
    let result = db.execute_cql("CREATE USER readonly WITH PASSWORD 'pass456'").await?;
    println!("CREATE USER readonly: {:?}", result);
    
    // LIST USERS
    let result = db.execute_cql("LIST USERS").await?;
    println!("\nLIST USERS: {:?}", result);
    
    // ALTER USER
    let result = db.execute_cql("ALTER USER readonly WITH PASSWORD 'newpass789'").await?;
    println!("\nALTER USER readonly: {:?}", result);
    
    // CREATE ROLE
    let result = db.execute_cql("CREATE ROLE developers WITH SUPERUSER = false AND LOGIN = true").await?;
    println!("\nCREATE ROLE developers: {:?}", result);
    
    // GRANT
    let result = db.execute_cql("GRANT SELECT ON ALL KEYSPACES TO developers").await?;
    println!("GRANT SELECT: {:?}", result);
    
    // LIST ROLES
    let result = db.execute_cql("LIST ROLES").await?;
    println!("\nLIST ROLES: {:?}", result);
    
    // LIST PERMISSIONS
    let result = db.execute_cql("LIST PERMISSIONS OF developers").await?;
    println!("\nLIST PERMISSIONS: {:?}", result);
    
    println!("\n=== Testing DESCRIBE ===\n");
    
    // DESCRIBE KEYSPACES (system keyspaces exist by default)
    let result = db.execute_cql("DESCRIBE KEYSPACES").await?;
    println!("DESCRIBE KEYSPACES: {:?}", result);
    
    // Create unique test keyspace
    let ks_name = format!("test_ks_{}", std::process::id());
    db.execute_cql(&format!("CREATE KEYSPACE {} WITH REPLICATION = {{'class': 'SimpleStrategy', 'replication_factor': 1}}", ks_name)).await?;
    db.execute_cql(&format!("CREATE TABLE {}.users (id INT PRIMARY KEY, name TEXT, age INT)", ks_name)).await?;
    println!("Created keyspace and table: {}.users", ks_name);
    
    // DESCRIBE KEYSPACE
    let result = db.execute_cql(&format!("DESCRIBE KEYSPACE {}", ks_name)).await?;
    println!("\nDESCRIBE KEYSPACE {}: {:?}", ks_name, result);
    
    // DESCRIBE TABLES
    let result = db.execute_cql(&format!("DESC TABLES {}", ks_name)).await?;
    println!("\nDESC TABLES: {:?}", result);
    
    // DESCRIBE TABLE
    let result = db.execute_cql(&format!("DESC {}.users", ks_name)).await?;
    println!("\nDESC {}.users: {:?}", ks_name, result);
    
    println!("\n✅ All tests passed!");
    Ok(())
}
