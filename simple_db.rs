use std::collections::HashMap;

/// 간단한 데이터베이스 구조
#[derive(Debug)]
pub struct SimpleDB {
    data: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

impl SimpleDB {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }
    
    pub fn create_keyspace(&mut self, name: &str) {
        self.data.insert(name.to_string(), HashMap::new());
        println!("✓ Created keyspace: {}", name);
    }
    
    pub fn create_table(&mut self, keyspace: &str, table: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            ks.insert(table.to_string(), HashMap::new());
            println!("✓ Created table: {}.{}", keyspace, table);
        } else {
            println!("✗ Keyspace '{}' not found", keyspace);
        }
    }
    
    pub fn insert(&mut self, keyspace: &str, table: &str, key: &str, value: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            if let Some(tbl) = ks.get_mut(table) {
                tbl.insert(key.to_string(), value.to_string());
                println!("✓ Inserted: {}.{}.{} = {}", keyspace, table, key, value);
            } else {
                println!("✗ Table '{}.{}' not found", keyspace, table);
            }
        } else {
            println!("✗ Keyspace '{}' not found", keyspace);
        }
    }
    
    pub fn get(&self, keyspace: &str, table: &str, key: &str) -> Option<String> {
        self.data
            .get(keyspace)?
            .get(table)?
            .get(key)
            .map(|v| v.clone())
    }
    
    pub fn list_keyspaces(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }
    
    pub fn list_tables(&self, keyspace: &str) -> Vec<String> {
        self.data
            .get(keyspace)
            .map(|ks| ks.keys().cloned().collect())
            .unwrap_or_default()
    }
    
    pub fn list_keys(&self, keyspace: &str, table: &str) -> Vec<String> {
        self.data
            .get(keyspace)
            .and_then(|ks| ks.get(table))
            .map(|tbl| tbl.keys().cloned().collect())
            .unwrap_or_default()
    }
    
    pub fn get_stats(&self) -> DatabaseStats {
        let mut total_tables = 0;
        let mut total_keys = 0;
        
        for keyspace in self.data.values() {
            total_tables += keyspace.len();
            for table in keyspace.values() {
                total_keys += table.len();
            }
        }
        
        DatabaseStats {
            keyspace_count: self.data.len(),
            table_count: total_tables,
            total_keys,
        }
    }
}

#[derive(Debug)]
pub struct DatabaseStats {
    pub keyspace_count: usize,
    pub table_count: usize,
    pub total_keys: usize,
}

fn main() {
    println!("🚀 CoreDB - Simple Cassandra-like Database Demo");
    println!("===============================================");
    
    let mut db = SimpleDB::new();
    
    // 키스페이스 생성
    println!("\n📁 Creating keyspaces...");
    db.create_keyspace("demo");
    db.create_keyspace("system");
    
    // 테이블 생성
    println!("\n📋 Creating tables...");
    db.create_table("demo", "users");
    db.create_table("demo", "products");
    db.create_table("system", "metadata");
    
    // 데이터 삽입
    println!("\n📝 Inserting data...");
    db.insert("demo", "users", "1", "John Doe");
    db.insert("demo", "users", "2", "Jane Smith");
    db.insert("demo", "users", "3", "Bob Johnson");
    
    db.insert("demo", "products", "p1", "Laptop");
    db.insert("demo", "products", "p2", "Mouse");
    db.insert("demo", "products", "p3", "Keyboard");
    
    db.insert("system", "metadata", "version", "1.0.0");
    db.insert("system", "metadata", "build_date", "2024-01-01");
    
    // 데이터 조회
    println!("\n🔍 Retrieving data...");
    println!("Users:");
    for i in 1..=3 {
        if let Some(value) = db.get("demo", "users", &i.to_string()) {
            println!("  User {}: {}", i, value);
        }
    }
    
    println!("\nProducts:");
    for key in db.list_keys("demo", "products") {
        if let Some(value) = db.get("demo", "products", &key) {
            println!("  {}: {}", key, value);
        }
    }
    
    println!("\nSystem metadata:");
    for key in db.list_keys("system", "metadata") {
        if let Some(value) = db.get("system", "metadata", &key) {
            println!("  {}: {}", key, value);
        }
    }
    
    // 통계 출력
    println!("\n📊 Database statistics:");
    let stats = db.get_stats();
    println!("  Keyspaces: {}", stats.keyspace_count);
    println!("  Tables: {}", stats.table_count);
    println!("  Total keys: {}", stats.total_keys);
    
    // 구조 출력
    println!("\n🏗️  Database structure:");
    for keyspace in db.list_keyspaces() {
        println!("  📁 {}", keyspace);
        for table in db.list_tables(&keyspace) {
            println!("    📋 {}.{} ({} keys)", keyspace, table, db.list_keys(&keyspace, &table).len());
        }
    }
    
    println!("\n✅ CoreDB demo completed successfully!");
    println!("This demonstrates the basic structure of a Cassandra-like database:");
    println!("- Keyspaces (like databases)");
    println!("- Tables (collections of key-value pairs)");
    println!("- Key-value storage within tables");
    println!("- Hierarchical organization (keyspace > table > key > value)");
}
