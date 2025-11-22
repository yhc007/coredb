# CoreDB CQLSH - Command Line Interface

## Overview

CQLSH is an interactive command-line interface for CoreDB, similar to Apache Cassandra's `cqlsh`. It provides a REPL (Read-Eval-Print Loop) environment for executing CQL queries and managing your CoreDB instance.

## Features

- ✅ **Interactive REPL** - Execute CQL queries interactively
- ✅ **Command History** - Navigate previous commands with arrow keys
- ✅ **Multi-line Queries** - Support for queries spanning multiple lines
- ✅ **Formatted Output** - Pretty-printed table results
- ✅ **Color-coded Messages** - Success in green, errors in red
- ✅ **Special Commands** - DESCRIBE, HELP, USE, EXIT
- ✅ **Auto-save History** - Command history persists across sessions

## Installation

Build the CQLSH binary:

```bash
cargo build --release --bin cqlsh
```

## Usage

Start CQLSH:

```bash
cargo run --bin cqlsh
# or
./target/release/cqlsh
```

## CQL Commands

### Keyspace Management

```cql
-- Create a keyspace
CREATE KEYSPACE my_keyspace WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};

-- Use a keyspace
USE my_keyspace;

-- Drop a keyspace
DROP KEYSPACE my_keyspace;
```

### Table Management

```cql
-- Create a table
CREATE TABLE my_keyspace.users (
    id INT PRIMARY KEY,
    name TEXT,
    age INT
);

-- Drop a table
DROP TABLE my_keyspace.users;
```

### Data Manipulation

```cql
-- Insert data
INSERT INTO my_keyspace.users (id, name, age) VALUES (1, 'Alice', 30);
INSERT INTO my_keyspace.users (id, name, age) VALUES (2, 'Bob', 25);

-- Query data
SELECT * FROM my_keyspace.users WHERE id = 1;
```

## Special Commands

### DESCRIBE Commands

```cql
-- List all keyspaces
DESCRIBE KEYSPACES;

-- List tables in current keyspace
DESCRIBE TABLES;

-- Show table schema
DESCRIBE TABLE users;
```

### Other Commands

```
HELP          -- Show help message
EXIT / QUIT   -- Exit CQLSH
```

## Example Session

```
$ cargo run --bin cqlsh
CoreDB Shell (CQLSH) v0.1.0
Type 'HELP' for help, 'EXIT' to quit.

cqlsh> CREATE KEYSPACE store WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};
✓ Success

cqlsh> USE store;
✓ Using keyspace 'store'

cqlsh:store> CREATE TABLE store.products (id INT PRIMARY KEY, name TEXT, price INT);
✓ Success

cqlsh:store> INSERT INTO store.products (id, name, price) VALUES (1, 'Laptop', 1200);
✓ Success

cqlsh:store> SELECT * FROM store.products WHERE id = 1;
+--------+----------------+------------+
| id     | name           | price      |
+--------+----------------+------------+
| Int(1) | Text("Laptop") | Int(1200)  |
+--------+----------------+------------+

1 row(s) returned

cqlsh:store> DESCRIBE TABLE products;
Table: store.products

+-------------+------+---------------+
| Column Name | Type | Kind          |
+-------------+------+---------------+
| id          | Int  | PARTITION KEY |
+-------------+------+---------------+
| name        | Text | REGULAR       |
+-------------+------+---------------+
| price       | Int  | REGULAR       |
+-------------+------+---------------+

cqlsh:store> EXIT
Goodbye!
```

## Tips

- **Multi-line Queries**: Queries must end with a semicolon (`;`). You can write queries across multiple lines.
- **Command History**: Use ↑ and ↓ arrow keys to navigate through previous commands.
- **History File**: Command history is saved to `.cqlsh_history` in the current directory.
- **Keyboard Shortcuts**:
  - `Ctrl+C` - Cancel current input
  - `Ctrl+D` - Exit CQLSH

## Limitations

- WHERE clause currently supports single conditions only (no AND/OR)
- Auto-completion not yet implemented
- Limited data type support compared to full Cassandra

## Data Storage

CQLSH stores data in the `./cqlsh_data` directory by default. This includes:
- Commit logs in `./cqlsh_data/commitlog`
- SSTable files organized by keyspace and table

## Troubleshooting

### Command not parsing

Make sure your CQL syntax is correct:
- Keyspace and table names must be specified as `keyspace.table`
- Queries must end with semicolon
- String values must be in single quotes

### Data not persisting

Data is written to commit logs immediately but may not be flushed to SSTables until shutdown or when memtable size threshold is reached.

## Development

The CQLSH implementation is located in `src/bin/cqlsh.rs` and uses:
- `rustyline` for line editing and history
- `prettytable-rs` for formatted table output
- `colored` for colored terminal output
