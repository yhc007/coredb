use std::collections::{HashMap, BTreeMap};
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{Write, Read, BufReader, BufWriter};
use std::path::Path;

// 간단한 데이터 타입
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SimpleValue {
    Text(String),
    Int(i64),
}

// 간단한 직렬화/역직렬화
impl SimpleValue {
    fn to_string(&self) -> String {
        match self {
            SimpleValue::Text(s) => format!("TEXT:{}", s),
            SimpleValue::Int(i) => format!("INT:{}", i),
        }
    }
    
    fn from_string(s: &str) -> Option<Self> {
        if let Some(text) = s.strip_prefix("TEXT:") {
            Some(SimpleValue::Text(text.to_string()))
        } else if let Some(num) = s.strip_prefix("INT:") {
            num.parse::<i64>().ok().map(SimpleValue::Int)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimpleRow {
    pub key: SimpleValue,
    pub value: SimpleValue,
    pub timestamp: i64,
}

#[derive(Debug)]
pub struct SimpleTable {
    pub name: String,
    data: BTreeMap<SimpleValue, SimpleRow>,
}

impl SimpleTable {
    pub fn new(name: String) -> Self {
        SimpleTable {
            name,
            data: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: SimpleValue, value: SimpleValue) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64;
        let row = SimpleRow { key: key.clone(), value, timestamp };
        self.data.insert(key, row);
    }

    pub fn get(&self, key: &SimpleValue) -> Option<SimpleRow> {
        self.data.get(key).cloned()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SimpleValue, &SimpleRow)> {
        self.data.iter()
    }
    
    // 텍스트 형식으로 저장
    fn save_to_string(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("TABLE:{}", self.name));
        for (key, row) in &self.data {
            lines.push(format!("ROW:{}|{}|{}", 
                key.to_string(), 
                row.value.to_string(), 
                row.timestamp
            ));
        }
        lines.join("\n")
    }
    
    // 텍스트 형식에서 로드
    fn load_from_lines(lines: &[String]) -> Option<Self> {
        if lines.is_empty() {
            return None;
        }
        
        let name = lines[0].strip_prefix("TABLE:")?.to_string();
        let mut table = SimpleTable::new(name);
        
        for line in &lines[1..] {
            if let Some(row_data) = line.strip_prefix("ROW:") {
                let parts: Vec<&str> = row_data.split('|').collect();
                if parts.len() == 3 {
                    if let (Some(key), Some(value), Ok(timestamp)) = (
                        SimpleValue::from_string(parts[0]),
                        SimpleValue::from_string(parts[1]),
                        parts[2].parse::<i64>()
                    ) {
                        let row = SimpleRow { key: key.clone(), value, timestamp };
                        table.data.insert(key, row);
                    }
                }
            }
        }
        
        Some(table)
    }
}

#[derive(Debug)]
pub struct SimpleKeyspace {
    pub name: String,
    tables: HashMap<String, SimpleTable>,
}

impl SimpleKeyspace {
    pub fn new(name: String) -> Self {
        SimpleKeyspace {
            name,
            tables: HashMap::new(),
        }
    }

    pub fn create_table(&mut self, table_name: String) {
        let table = SimpleTable::new(table_name.clone());
        self.tables.insert(table_name, table);
    }

    pub fn get_table(&mut self, table_name: &str) -> Option<&mut SimpleTable> {
        self.tables.get_mut(table_name)
    }

    pub fn tables_count(&self) -> usize {
        self.tables.len()
    }
    
    fn save_to_string(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("KEYSPACE:{}", self.name));
        for table in self.tables.values() {
            lines.push(table.save_to_string());
        }
        lines.join("\n")
    }
    
    fn load_from_lines(lines: &[String]) -> Option<Self> {
        if lines.is_empty() {
            return None;
        }
        
        let name = lines[0].strip_prefix("KEYSPACE:")?.to_string();
        let mut keyspace = SimpleKeyspace::new(name);
        
        let mut current_table_lines = Vec::new();
        for line in &lines[1..] {
            if line.starts_with("TABLE:") {
                if !current_table_lines.is_empty() {
                    if let Some(table) = SimpleTable::load_from_lines(&current_table_lines) {
                        keyspace.tables.insert(table.name.clone(), table);
                    }
                    current_table_lines.clear();
                }
            }
            current_table_lines.push(line.clone());
        }
        
        if !current_table_lines.is_empty() {
            if let Some(table) = SimpleTable::load_from_lines(&current_table_lines) {
                keyspace.tables.insert(table.name.clone(), table);
            }
        }
        
        Some(keyspace)
    }
}

#[derive(Debug)]
pub struct SimplePersistentDB {
    keyspaces: HashMap<String, SimpleKeyspace>,
    data_directory: String,
}

impl SimplePersistentDB {
    pub fn new(data_directory: String) -> Self {
        create_dir_all(&data_directory).expect("Failed to create data directory");
        
        SimplePersistentDB {
            keyspaces: HashMap::new(),
            data_directory,
        }
    }

    pub fn create_keyspace(&mut self, keyspace_name: String) {
        let ks = SimpleKeyspace::new(keyspace_name.clone());
        self.keyspaces.insert(keyspace_name, ks);
    }

    pub fn get_keyspace(&mut self, keyspace_name: &str) -> Option<&mut SimpleKeyspace> {
        self.keyspaces.get_mut(keyspace_name)
    }

    pub fn keyspace_count(&self) -> usize {
        self.keyspaces.len()
    }

    pub fn total_tables(&self) -> usize {
        self.keyspaces.values().map(|ks| ks.tables_count()).sum()
    }

    pub fn total_keys(&self) -> usize {
        self.keyspaces.values()
            .flat_map(|ks| ks.tables.values())
            .map(|tbl| tbl.len())
            .sum()
    }

    /// 데이터베이스를 텍스트 파일로 저장
    pub fn save_to_disk(&self) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = format!("{}/db_snapshot.txt", self.data_directory);
        println!("💾 Saving database to: {}", file_path);
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)?;
        
        writeln!(file, "# CoreDB Simple Persistent Database")?;
        writeln!(file, "# Format: KEYSPACE > TABLE > ROW")?;
        writeln!(file, "")?;
        
        for keyspace in self.keyspaces.values() {
            writeln!(file, "{}", keyspace.save_to_string())?;
            writeln!(file, "")?;
        }
        
        println!("✅ Database saved successfully!");
        Ok(())
    }

    /// 텍스트 파일에서 데이터베이스 복구
    pub fn load_from_disk(data_directory: String) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("{}/db_snapshot.txt", data_directory);
        
        if !Path::new(&file_path).exists() {
            println!("⚠️  No snapshot found, creating new database");
            return Ok(Self::new(data_directory));
        }
        
        println!("📂 Loading database from: {}", file_path);
        
        let file = File::open(&file_path)?;
        let reader = BufReader::new(file);
        
        let mut db = SimplePersistentDB::new(data_directory);
        let mut lines = Vec::new();
        
        use std::io::BufRead;
        for line in reader.lines() {
            let line = line?;
            if !line.starts_with('#') && !line.trim().is_empty() {
                lines.push(line);
            }
        }
        
        let mut current_keyspace_lines = Vec::new();
        for line in &lines {
            if line.starts_with("KEYSPACE:") {
                if !current_keyspace_lines.is_empty() {
                    if let Some(ks) = SimpleKeyspace::load_from_lines(&current_keyspace_lines) {
                        db.keyspaces.insert(ks.name.clone(), ks);
                    }
                    current_keyspace_lines.clear();
                }
            }
            current_keyspace_lines.push(line.clone());
        }
        
        if !current_keyspace_lines.is_empty() {
            if let Some(ks) = SimpleKeyspace::load_from_lines(&current_keyspace_lines) {
                db.keyspaces.insert(ks.name.clone(), ks);
            }
        }
        
        println!("✅ Database loaded successfully!");
        println!("   Keyspaces: {}", db.keyspace_count());
        println!("   Tables: {}", db.total_tables());
        println!("   Total keys: {}", db.total_keys());
        
        Ok(db)
    }

    /// Write-Ahead Log 쓰기
    pub fn write_wal(&self, operation: &str) -> Result<(), Box<dyn std::error::Error>> {
        let wal_path = format!("{}/wal.log", self.data_directory);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(wal_path)?;
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        writeln!(file, "{}\t{}", timestamp, operation)?;
        
        Ok(())
    }
}

fn main() {
    println!("🚀 CoreDB - Simple Persistent Database Demo");
    println!("==========================================\n");

    let data_dir = "./data".to_string();

    // 1. 기존 데이터 로드 또는 새 DB 생성
    println!("1️⃣  LOADING DATABASE");
    let mut db = SimplePersistentDB::load_from_disk(data_dir.clone())
        .unwrap_or_else(|e| {
            println!("⚠️  Load failed: {}, creating new database", e);
            SimplePersistentDB::new(data_dir.clone())
        });

    // 2. 키스페이스 생성 (없으면)
    println!("\n2️⃣  MANAGING KEYSPACES");
    if db.keyspace_count() == 0 {
        db.create_keyspace("demo".to_string());
        println!("✓ Created keyspace: demo");
        db.create_keyspace("system".to_string());
        println!("✓ Created keyspace: system");
    } else {
        println!("✓ Using existing keyspaces: {}", db.keyspace_count());
    }

    // 3. 테이블 생성 (없으면)
    println!("\n3️⃣  MANAGING TABLES");
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if demo_ks.tables_count() == 0 {
            demo_ks.create_table("users".to_string());
            println!("✓ Created table: demo.users");
            demo_ks.create_table("products".to_string());
            println!("✓ Created table: demo.products");
        } else {
            println!("✓ Using existing tables: {}", demo_ks.tables_count());
        }
    }
    
    if let Some(system_ks) = db.get_keyspace("system") {
        if system_ks.tables_count() == 0 {
            system_ks.create_table("metadata".to_string());
            println!("✓ Created table: system.metadata");
        }
    }

    // 4. 데이터 삽입
    println!("\n4️⃣  INSERTING DATA");
    let new_user_count = 3;
    let mut wal_operations = Vec::new();
    
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if let Some(users_table) = demo_ks.get_table("users") {
            let current_count = users_table.len();
            
            for i in 1..=new_user_count {
                let user_id = current_count + i;
                users_table.insert(
                    SimpleValue::Int(user_id as i64),
                    SimpleValue::Text(format!("User #{}", user_id))
                );
                println!("✓ Inserted: demo.users.{} = User #{}", user_id, user_id);
                wal_operations.push(format!("INSERT demo.users {} User#{}", user_id, user_id));
            }
        }

        if let Some(products_table) = demo_ks.get_table("products") {
            if products_table.is_empty() {
                products_table.insert(
                    SimpleValue::Text("p1".to_string()),
                    SimpleValue::Text("Laptop".to_string())
                );
                println!("✓ Inserted: demo.products.p1 = Laptop");
                wal_operations.push("INSERT demo.products p1 Laptop".to_string());
                
                products_table.insert(
                    SimpleValue::Text("p2".to_string()),
                    SimpleValue::Text("Mouse".to_string())
                );
                println!("✓ Inserted: demo.products.p2 = Mouse");
                wal_operations.push("INSERT demo.products p2 Mouse".to_string());
            } else {
                println!("✓ Products table already has data");
            }
        }
    }
    
    // WAL 기록
    for operation in wal_operations {
        db.write_wal(&operation).ok();
    }

    // 5. 데이터 조회
    println!("\n5️⃣  RETRIEVING DATA");
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if let Some(users_table) = demo_ks.get_table("users") {
            println!("Users (total: {}):", users_table.len());
            for (key, row) in users_table.iter().take(10) {
                println!("  {:?}: {:?} (timestamp: {})", key, row.value, row.timestamp);
            }
            if users_table.len() > 10 {
                println!("  ... and {} more", users_table.len() - 10);
            }
        }

        if let Some(products_table) = demo_ks.get_table("products") {
            println!("\nProducts:");
            for (key, row) in products_table.iter() {
                println!("  {:?}: {:?}", key, row.value);
            }
        }
    }

    // 6. 데이터베이스 상태
    println!("\n6️⃣  DATABASE STATISTICS");
    println!("  Keyspaces: {}", db.keyspace_count());
    println!("  Tables: {}", db.total_tables());
    println!("  Total keys: {}", db.total_keys());

    // 7. 디스크에 저장
    println!("\n7️⃣  SAVING TO DISK");
    match db.save_to_disk() {
        Ok(_) => println!("✅ Snapshot saved successfully"),
        Err(e) => println!("❌ Save failed: {}", e),
    }

    println!("\n📁 Data files:");
    println!("  - {}/db_snapshot.txt (human-readable)", data_dir);
    println!("  - {}/wal.log (write-ahead log)", data_dir);

    println!("\n✅ CoreDB persistent demo completed!");
    println!("💡 Tip: Run this program again to see data persistence in action!");
    println!("\n🔄 To see persistence:");
    println!("   $ rustc simple_persistent_db.rs -o simple_persistent_db");
    println!("   $ ./simple_persistent_db  # First run - creates data");
    println!("   $ ./simple_persistent_db  # Second run - loads existing data!");
}

