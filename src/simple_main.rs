use std::path::PathBuf;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

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
        println!("Created keyspace: {}", name);
    }
    
    pub fn create_table(&mut self, keyspace: &str, table: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            ks.insert(table.to_string(), HashMap::new());
            println!("Created table: {}.{}", keyspace, table);
        } else {
            println!("Keyspace '{}' not found", keyspace);
        }
    }
    
    pub fn insert(&mut self, keyspace: &str, table: &str, key: &str, value: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            if let Some(tbl) = ks.get_mut(table) {
                tbl.insert(key.to_string(), value.to_string());
                println!("Inserted: {}.{}.{} = {}", keyspace, table, key, value);
            } else {
                println!("Table '{}.{}' not found", keyspace, table);
            }
        } else {
            println!("Keyspace '{}' not found", keyspace);
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
}

fn main() {
    println!("CoreDB - Simple Cassandra-like Database");
    println!("======================================");
    
    let mut db = SimpleDB::new();
    
    // 키스페이스 생성
    db.create_keyspace("demo");
    
    // 테이블 생성
    db.create_table("demo", "users");
    
    // 데이터 삽입
    db.insert("demo", "users", "1", "John Doe");
    db.insert("demo", "users", "2", "Jane Smith");
    db.insert("demo", "users", "3", "Bob Johnson");
    
    // 데이터 조회
    println!("\nData retrieval:");
    for i in 1..=3 {
        if let Some(value) = db.get("demo", "users", &i.to_string()) {
            println!("User {}: {}", i, value);
        }
    }
    
    // 키스페이스 목록
    println!("\nKeyspaces: {:?}", db.list_keyspaces());
    
    // 테이블 목록
    println!("Tables in 'demo': {:?}", db.list_tables("demo"));
    
    // 키 목록
    println!("Keys in 'demo.users': {:?}", db.list_keys("demo", "users"));
    
    println!("\nSimple CoreDB demo completed!");
}
