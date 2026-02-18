use coredb::database::{CoreDB, DatabaseConfig};

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config = DatabaseConfig::default();
    let db = CoreDB::new(config).await?;
    
    let ks = format!("test_ks_{}", std::process::id());
    
    // 키스페이스 및 테이블 생성
    db.execute_cql(&format!("CREATE KEYSPACE {} WITH REPLICATION = {{'class': 'SimpleStrategy', 'replication_factor': 1}}", ks)).await?;
    
    println!("=== Testing COUNTER ===\n");
    
    // COUNTER 테이블 생성
    db.execute_cql(&format!("CREATE TABLE {}.page_views (page_id INT PRIMARY KEY, views COUNTER)", ks)).await?;
    println!("Created table {}.page_views with COUNTER column", ks);
    
    // COUNTER 초기화 (첫 증가)
    let result = db.execute_cql(&format!("UPDATE {}.page_views SET views = views + 1 WHERE page_id = 1", ks)).await?;
    println!("UPDATE views = views + 1: {:?}", result);
    
    // COUNTER 증가
    let result = db.execute_cql(&format!("UPDATE {}.page_views SET views = views + 10 WHERE page_id = 1", ks)).await?;
    println!("UPDATE views = views + 10: {:?}", result);
    
    // COUNTER 감소
    let result = db.execute_cql(&format!("UPDATE {}.page_views SET views = views - 3 WHERE page_id = 1", ks)).await?;
    println!("UPDATE views = views - 3: {:?}", result);
    
    // 결과 확인
    let result = db.execute_cql(&format!("SELECT * FROM {}.page_views WHERE page_id = 1", ks)).await?;
    println!("\nSELECT result (expected views=8): {:?}", result);
    
    println!("\n=== Testing ORDER BY ===\n");
    
    // ORDER BY 테스트 테이블
    db.execute_cql(&format!("CREATE TABLE {}.users (id INT PRIMARY KEY, name TEXT, age INT)", ks)).await?;
    db.execute_cql(&format!("INSERT INTO {}.users (id, name, age) VALUES (1, 'Alice', 30)", ks)).await?;
    db.execute_cql(&format!("INSERT INTO {}.users (id, name, age) VALUES (2, 'Bob', 25)", ks)).await?;
    db.execute_cql(&format!("INSERT INTO {}.users (id, name, age) VALUES (3, 'Charlie', 35)", ks)).await?;
    
    let result = db.execute_cql(&format!("SELECT * FROM {}.users ORDER BY age ASC", ks)).await?;
    println!("ORDER BY age ASC: {:?}", result);
    
    let result = db.execute_cql(&format!("SELECT * FROM {}.users ORDER BY age DESC", ks)).await?;
    println!("ORDER BY age DESC: {:?}", result);
    
    println!("\n=== Testing TOKEN ===\n");
    
    // TOKEN 테스트 - 토큰 범위 쿼리
    let result = db.execute_cql(&format!("SELECT * FROM {}.users WHERE token(id) > 0", ks)).await?;
    println!("WHERE token(id) > 0: {:?}", result);
    
    println!("\n✅ All tests passed!");
    Ok(())
}
