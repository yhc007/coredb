-- Secondary Index + TTL 테스트

-- 1. Keyspace 생성
CREATE KEYSPACE test_features WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};

-- 2. 테이블 생성
CREATE TABLE test_features.users (id INT PRIMARY KEY, name TEXT, email TEXT, age INT);

-- 3. 데이터 삽입
INSERT INTO test_features.users (id, name, email, age) VALUES (1, 'John', 'john@example.com', 30);
INSERT INTO test_features.users (id, name, email, age) VALUES (2, 'Jane', 'jane@example.com', 25);
INSERT INTO test_features.users (id, name, email, age) VALUES (3, 'Bob', 'bob@example.com', 35);

-- 4. Secondary Index 생성
CREATE INDEX idx_email ON test_features.users (email);

-- 5. 일반 조회
SELECT * FROM test_features.users WHERE id = 1;

-- 6. TTL 테스트용 테이블
CREATE TABLE test_features.sessions (id INT PRIMARY KEY, token TEXT);

-- 7. TTL 5초로 데이터 삽입
INSERT INTO test_features.sessions (id, token) VALUES (1, 'temp_token') USING TTL 5;

-- 8. 즉시 조회 (데이터 있어야 함)
SELECT * FROM test_features.sessions WHERE id = 1;
