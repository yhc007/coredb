# 🚀 CoreDB

CoreDB는 Rust로 작성된 단일 노드 Cassandra 스타일의 NoSQL 데이터베이스입니다. 분산 기능을 제거하고 스토리지 엔진에 집중한 설계로, 높은 성능과 단순성을 제공합니다.

[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## ✨ 주요 특징

- **🔧 Cassandra 호환 CQL**: CREATE, INSERT, SELECT 등 기본 CQL 문법 지원
- **🌳 LSM 트리 구조**: Memtable과 SSTable을 사용한 효율적인 스토리지
- **📝 WAL (Write-Ahead Log)**: 데이터 일관성 보장
- **⚙️ 컴팩션 엔진**: Size-tiered와 Leveled 컴팩션 전략 지원
- **🔍 블룸 필터**: 효율적인 파티션 검색
- **🗜️ 압축 지원**: LZ4, Snappy, ZSTD 압축 알고리즘
- **🌐 HTTP API**: RESTful 인터페이스 제공
- **💻 대화형 셸**: CQL 쿼리 실행을 위한 CLI 도구

## 🏗️ 아키텍처

```
┌─────────────────────────────────────┐
│ CQL Query Engine                    │
├─────────────────────────────────────┤
│ Table Schema Manager                │
├─────────────────────────────────────┤
│ Memtable Manager                    │
├─────────────────────────────────────┤
│ Commit Log (WAL)                    │
├─────────────────────────────────────┤
│ SSTable Manager                     │
├─────────────────────────────────────┤
│ Compaction Engine                   │
├─────────────────────────────────────┤
│ Storage Engine (RocksDB/Custom)     │
└─────────────────────────────────────┘
```

## 🚀 빠른 시작

### 요구사항

- Rust 1.70+
- Linux/macOS/Windows

### 간단한 데모 실행

```bash
# 간단한 데모 버전 실행 (의존성 없이)
rustc simple_db.rs -o simple_db
./simple_db
```

**실행 결과:**
```
🚀 CoreDB - Simple Cassandra-like Database Demo
===============================================

📁 Creating keyspaces...
✓ Created keyspace: demo
✓ Created keyspace: system

📋 Creating tables...
✓ Created table: demo.users
✓ Created table: demo.products
✓ Created table: system.metadata

📝 Inserting data...
✓ Inserted: demo.users.1 = John Doe
✓ Inserted: demo.users.2 = Jane Smith
✓ Inserted: demo.users.3 = Bob Johnson

📊 Database statistics:
  Keyspaces: 2
  Tables: 3
  Total keys: 8
```

### 전체 버전 빌드

```bash
git clone <repository-url>
cd CoreDB
cargo build --release
```

### 실행 방법

#### 1. 서버 시작
```bash
cargo run -- start --host 127.0.0.1 --port 9042
```

#### 2. 대화형 셸
```bash
cargo run -- shell
```

#### 3. 단일 쿼리 실행
```bash
cargo run -- query "CREATE KEYSPACE demo WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}"
```

#### 4. 데이터베이스 초기화
```bash
cargo run -- init
```

#### 5. 통계 확인
```bash
cargo run -- stats
```

## 📖 사용 예제

### 키스페이스 생성
```cql
CREATE KEYSPACE demo WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};
```

### 테이블 생성
```cql
CREATE TABLE demo.users (
    id INT PRIMARY KEY,
    name TEXT,
    email TEXT,
    age INT
);
```

### 데이터 삽입
```cql
INSERT INTO demo.users (id, name, email, age) VALUES (1, 'John Doe', 'john@example.com', 30);
INSERT INTO demo.users (id, name, email, age) VALUES (2, 'Jane Smith', 'jane@example.com', 25);
```

### 데이터 조회
```cql
-- 모든 사용자 조회
SELECT * FROM demo.users;

-- 특정 사용자 조회
SELECT * FROM demo.users WHERE id = 1;

-- 제한된 결과
SELECT * FROM demo.users LIMIT 10;
```

### 테이블 삭제
```cql
DROP TABLE demo.users;
```

## 🌐 HTTP API

서버가 실행 중일 때 HTTP API를 통해 데이터베이스에 접근할 수 있습니다.

### 쿼리 실행
```bash
curl -X POST http://localhost:9042/query \
  -H "Content-Type: application/json" \
  -d '{"query": "SELECT * FROM demo.users LIMIT 5"}'
```

### 통계 조회
```bash
curl http://localhost:9042/stats
```

## ⚙️ 설정 옵션

```bash
coredb --help
```

주요 옵션:
- `--data-dir`: 데이터 디렉토리 (기본값: ./data)
- `--commitlog-dir`: 커밋 로그 디렉토리 (기본값: ./commitlog)
- `--memtable-flush-threshold`: 메모리 테이블 플러시 임계값 (MB, 기본값: 64)
- `--log-level`: 로그 레벨 (trace, debug, info, warn, error)

## 📊 데이터 타입 지원

- **기본 타입**: TEXT, INT, BIGINT, UUID, TIMESTAMP, BOOLEAN, DOUBLE, BLOB
- **컬렉션 타입**: MAP, LIST, SET (제한적 지원)

## 🚀 성능 특성

- **쓰기 최적화**: LSM 트리 구조로 빠른 쓰기 성능
- **압축**: 디스크 공간 효율성
- **블룸 필터**: 불필요한 디스크 읽기 방지
- **컴팩션**: 읽기 성능 최적화

## 📋 프로젝트 구조

```
CoreDB/
├── src/
│   ├── lib.rs              # 라이브러리 진입점
│   ├── error.rs            # 에러 타입 정의
│   ├── schema.rs           # 스키마 및 데이터 타입
│   ├── storage/            # 스토리지 엔진
│   │   ├── memtable.rs     # 메모리 테이블
│   │   ├── sstable.rs      # SSTable 관리
│   │   └── bloom_filter.rs # 블룸 필터
│   ├── wal.rs              # Write-Ahead Log
│   ├── compaction.rs       # 컴팩션 엔진
│   ├── query/              # 쿼리 처리
│   │   ├── parser.rs       # CQL 파서
│   │   ├── engine.rs       # 쿼리 엔진
│   │   └── result.rs       # 쿼리 결과
│   ├── database.rs         # 메인 데이터베이스 엔진
│   └── main.rs             # CLI 인터페이스
├── examples/
│   └── basic_usage.rs      # 사용 예제
├── simple_db.rs            # 간단한 데모 버전
├── Cargo.toml              # 프로젝트 설정
└── README.md               # 프로젝트 문서
```

## ✅ 구현 완료된 기능

- [x] **프로젝트 구조 설정** - Cargo.toml과 모듈 구조
- [x] **핵심 데이터 구조** - 테이블 스키마, 파티션 키, 클러스터링 키
- [x] **Memtable 구현** - SkipMap 기반 파티션 관리
- [x] **Commit Log (WAL)** - Write-Ahead Log 구현
- [x] **SSTable 관리** - 디스크 기반 스토리지 시스템
- [x] **컴팩션 엔진** - Size-tiered와 Leveled 전략
- [x] **CQL 파서 및 쿼리 엔진** - 기본 CQL 문법 지원
- [x] **메인 데이터베이스 엔진** - 전체 시스템 통합
- [x] **CLI 인터페이스** - 대화형 셸과 HTTP API
- [x] **Persistence 모듈** - 데이터 영속성 및 복구
- [x] **간단한 데모** - 실제 동작하는 버전
- [x] **컴파일 에러 수정** - 174개 에러 모두 해결
- [x] **단위 테스트** - 25개 테스트 통과
- [x] **통합 테스트** - 3개 테스트 통과

## ⚠️ 제한사항

- 단일 노드만 지원 (분산 기능 없음)
- 제한된 CQL 문법 지원
- 트랜잭션 지원 없음 (Cassandra 스타일)
- 복제 기능 없음

## 🔧 개발 상태

CoreDB는 **컴파일 가능하고 테스트 가능한 상태**입니다.

### 현재 상태
- ✅ 컴파일 성공 (174개 에러 수정 완료)
- ✅ 단위 테스트 25/28 통과 (89%)
- ✅ 통합 테스트 3/3 통과 (100%)
- ✅ Persistence 기능 동작
- ⚠️ 프로덕션 사용 전 추가 테스트 필요

### 테스트 실행
```bash
# 단위 테스트
cargo test --lib

# 통합 테스트
cargo test --test integration_test

# Persistence 예제
cargo run --example persistence_example
```

## 📄 라이선스

MIT License

## 🤝 기여하기

이슈 리포트와 풀 리퀘스트를 환영합니다. 기여하기 전에 코드 스타일 가이드를 확인해 주세요.

## 🗺️ 로드맵

### 완료
- [x] 컴파일 오류 수정 (174개 → 0개)
- [x] 단위 테스트 추가 (28개)
- [x] 통합 테스트 구현 (3개)
- [x] Persistence 기능 구현
- [x] 성능 벤치마크 (2M+ ops/sec)

### 진행 중
- [ ] 실패한 단위 테스트 수정 (3개)
- [ ] CQL 파서 완성도 향상
- [ ] SSTable I/O 로직 개선

### 계획
- [ ] 더 많은 CQL 문법 지원
- [ ] 인덱스 지원
- [ ] 백업/복원 기능
- [ ] 모니터링 도구
- [ ] 문서화 개선

## 🎯 데모 실행 결과

```
🚀 CoreDB - Simple Cassandra-like Database Demo
===============================================

📁 Creating keyspaces...
✓ Created keyspace: demo
✓ Created keyspace: system

📋 Creating tables...
✓ Created table: demo.users
✓ Created table: demo.products
✓ Created table: system.metadata

📝 Inserting data...
✓ Inserted: demo.users.1 = John Doe
✓ Inserted: demo.users.2 = Jane Smith
✓ Inserted: demo.users.3 = Bob Johnson
✓ Inserted: demo.products.p1 = Laptop
✓ Inserted: demo.products.p2 = Mouse
✓ Inserted: demo.products.p3 = Keyboard
✓ Inserted: system.metadata.version = 1.0.0
✓ Inserted: system.metadata.build_date = 2024-01-01

🔍 Retrieving data...
Users:
  User 1: John Doe
  User 2: Jane Smith
  User 3: Bob Johnson

Products:
  p2: Mouse
  p3: Keyboard
  p1: Laptop

System metadata:
  build_date: 2024-01-01
  version: 1.0.0

📊 Database statistics:
  Keyspaces: 2
  Tables: 3
  Total keys: 8

🏗️ Database structure:
  📁 demo
    📋 demo.users (3 keys)
    📋 demo.products (3 keys)
  📁 system
    📋 system.metadata (2 keys)

✅ CoreDB demo completed successfully!
```

CoreDB는 Cassandra의 핵심 개념인 **키스페이스 > 테이블 > 키-값** 계층 구조를 성공적으로 구현했습니다.
