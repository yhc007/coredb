#!/bin/bash

# CQLSH Test Script
# This script tests the CQLSH CLI functionality

echo "Testing CQLSH..."

# Create a test input file
cat > /tmp/cqlsh_test_input.cql << 'EOF'
HELP;
CREATE KEYSPACE test_ks WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};
DESCRIBE KEYSPACES;
USE test_ks;
CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT);
DESCRIBE TABLES;
DESCRIBE TABLE users;
INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30);
INSERT INTO users (id, name, age) VALUES (2, 'Bob', 25);
SELECT * FROM users WHERE id = 1;
SELECT * FROM users WHERE id = 2;
EXIT;
EOF

# Run CQLSH with the test input
cargo run --bin cqlsh < /tmp/cqlsh_test_input.cql

# Cleanup
rm -f /tmp/cqlsh_test_input.cql
rm -rf ./cqlsh_data

echo ""
echo "Test completed!"
