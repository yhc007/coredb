#!/bin/bash

# Simple CQLSH Test
echo "Simple CQLSH Test..."

cat > /tmp/simple_test.cql << 'EOF'
CREATE KEYSPACE test WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};
USE test;
CREATE TABLE test.users (id INT PRIMARY KEY, name TEXT);
INSERT INTO test.users (id, name) VALUES (1, 'Alice');
SELECT * FROM test.users WHERE id = 1;
EXIT;
EOF

cargo run --bin cqlsh < /tmp/simple_test.cql 2>&1 | grep -A 5 -B 2 "Error\|Success\|row"

rm -f /tmp/simple_test.cql
rm -rf ./cqlsh_data
