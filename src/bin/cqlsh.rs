use coredb::database::{CoreDB, DatabaseConfig};
use coredb::query::QueryResult;
use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, Result as RustylineResult};
use prettytable::{Table, Row as PrettyRow, Cell};
use colored::*;
use std::path::PathBuf;

struct CqlShell {
    db: CoreDB,
    current_keyspace: Option<String>,
}

impl CqlShell {
    async fn new(config: DatabaseConfig) -> anyhow::Result<Self> {
        let db = CoreDB::new(config).await?;
        Ok(Self {
            db,
            current_keyspace: None,
        })
    }

    async fn execute_line(&mut self, line: &str) -> anyhow::Result<String> {
        let trimmed = line.trim();
        
        // Handle special commands
        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            return Ok("EXIT".to_string());
        }
        
        if trimmed.eq_ignore_ascii_case("help") {
            return Ok(self.print_help());
        }
        
        if trimmed.to_uppercase().starts_with("DESCRIBE KEYSPACES") {
            return Ok(self.describe_keyspaces().await?);
        }
        
        if trimmed.to_uppercase().starts_with("DESCRIBE TABLES") {
            return Ok(self.describe_tables().await?);
        }
        
        if trimmed.to_uppercase().starts_with("DESCRIBE TABLE") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                return Ok(self.describe_table(parts[2]).await?);
            }
        }
        
        // Execute CQL query
        match self.db.execute_cql(trimmed).await {
            Ok(result) => {
                // Check if it's a USE statement to update current keyspace
                if trimmed.to_uppercase().starts_with("USE") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        self.current_keyspace = Some(parts[1].to_string());
                        return Ok(format!("✓ Using keyspace '{}'", parts[1]).green().to_string());
                    }
                }
                
                Ok(self.format_result(result))
            }
            Err(e) => Ok(format!("✗ Error: {}", e).red().to_string()),
        }
    }

    fn format_result(&self, result: QueryResult) -> String {
        match result {
            QueryResult::Success => "✓ Success".green().to_string(),
            QueryResult::Rows(rows) => {
                if rows.is_empty() {
                    return "0 row(s) returned".yellow().to_string();
                }
                
                let mut table = Table::new();
                
                // Get column names from first row
                let first_row = &rows[0];
                let mut column_names: Vec<String> = first_row.columns.keys().cloned().collect();
                column_names.sort(); // Sort for consistent ordering
                
                // Add header
                let header_cells: Vec<Cell> = column_names.iter()
                    .map(|name| Cell::new(name).style_spec("Fb"))
                    .collect();
                table.add_row(PrettyRow::new(header_cells));
                
                // Add data rows
                for row in &rows {
                    let cells: Vec<Cell> = column_names.iter()
                        .map(|name| {
                            let value = row.columns.get(name)
                                .map(|v| format!("{:?}", v))
                                .unwrap_or_else(|| "NULL".to_string());
                            Cell::new(&value)
                        })
                        .collect();
                    table.add_row(PrettyRow::new(cells));
                }
                
                format!("{}\n{} row(s) returned", table.to_string(), rows.len())
            }
            QueryResult::Schema(schema) => {
                format!("✓ Schema: {:?}", schema).green().to_string()
            }
            QueryResult::Error(msg) => {
                format!("✗ Error: {}", msg).red().to_string()
            }
        }
    }

    async fn describe_keyspaces(&self) -> anyhow::Result<String> {
        let stats = self.db.get_stats().await;
        let keyspaces = self.db.keyspaces.read().await;
        
        let mut table = Table::new();
        table.add_row(PrettyRow::new(vec![
            Cell::new("Keyspace Name").style_spec("Fb"),
            Cell::new("Replication Factor").style_spec("Fb"),
        ]));
        
        for (name, ks) in keyspaces.iter() {
            table.add_row(PrettyRow::new(vec![
                Cell::new(name),
                Cell::new(&ks.definition.replication_factor.to_string()),
            ]));
        }
        
        Ok(format!("{}\n{} keyspace(s) found", table.to_string(), keyspaces.len()))
    }

    async fn describe_tables(&self) -> anyhow::Result<String> {
        let keyspace_name = self.current_keyspace.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No keyspace selected. Use 'USE <keyspace>' first."))?;
        
        let keyspaces = self.db.keyspaces.read().await;
        let keyspace = keyspaces.get(keyspace_name)
            .ok_or_else(|| anyhow::anyhow!("Keyspace '{}' not found", keyspace_name))?;
        
        let tables = keyspace.tables.read().await;
        
        let mut table = Table::new();
        table.add_row(PrettyRow::new(vec![
            Cell::new("Table Name").style_spec("Fb"),
        ]));
        
        for name in tables.keys() {
            table.add_row(PrettyRow::new(vec![Cell::new(name)]));
        }
        
        Ok(format!("{}\n{} table(s) found in keyspace '{}'", 
                   table.to_string(), tables.len(), keyspace_name))
    }

    async fn describe_table(&self, table_name: &str) -> anyhow::Result<String> {
        let keyspace_name = self.current_keyspace.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No keyspace selected. Use 'USE <keyspace>' first."))?;
        
        let keyspaces = self.db.keyspaces.read().await;
        let keyspace = keyspaces.get(keyspace_name)
            .ok_or_else(|| anyhow::anyhow!("Keyspace '{}' not found", keyspace_name))?;
        
        let tables = keyspace.tables.read().await;
        let table = tables.get(table_name)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' not found", table_name))?;
        
        let schema = &table.schema;
        
        let mut output = String::new();
        output.push_str(&format!("Table: {}.{}\n\n", keyspace_name, table_name).bold().to_string());
        
        let mut col_table = Table::new();
        col_table.add_row(PrettyRow::new(vec![
            Cell::new("Column Name").style_spec("Fb"),
            Cell::new("Type").style_spec("Fb"),
            Cell::new("Kind").style_spec("Fb"),
        ]));
        
        // Partition keys
        for col in &schema.partition_key {
            col_table.add_row(PrettyRow::new(vec![
                Cell::new(&col.name),
                Cell::new(&format!("{:?}", col.data_type)),
                Cell::new("PARTITION KEY").style_spec("Fg"),
            ]));
        }
        
        // Clustering keys
        for col in &schema.clustering_key {
            col_table.add_row(PrettyRow::new(vec![
                Cell::new(&col.name),
                Cell::new(&format!("{:?}", col.data_type)),
                Cell::new("CLUSTERING KEY").style_spec("Fy"),
            ]));
        }
        
        // Regular columns
        for col in &schema.regular_columns {
            col_table.add_row(PrettyRow::new(vec![
                Cell::new(&col.name),
                Cell::new(&format!("{:?}", col.data_type)),
                Cell::new("REGULAR"),
            ]));
        }
        
        output.push_str(&col_table.to_string());
        Ok(output)
    }

    fn print_help(&self) -> String {
        let help_text = r#"
CoreDB Shell (CQLSH) - Help

CQL COMMANDS:
  CREATE KEYSPACE <name> WITH REPLICATION = {...}
  CREATE TABLE <keyspace>.<table> (...)
  INSERT INTO <keyspace>.<table> (...) VALUES (...)
  SELECT * FROM <keyspace>.<table> WHERE ...
  DROP TABLE <keyspace>.<table>
  DROP KEYSPACE <name>
  USE <keyspace>

SPECIAL COMMANDS:
  DESCRIBE KEYSPACES           - List all keyspaces
  DESCRIBE TABLES              - List tables in current keyspace
  DESCRIBE TABLE <name>        - Show table schema
  HELP                         - Show this help message
  EXIT / QUIT                  - Exit the shell

TIPS:
  - Use arrow keys to navigate command history
  - Queries must end with semicolon (;)
  - Use 'USE <keyspace>' to set current keyspace
"#;
        help_text.to_string()
    }

    fn get_prompt(&self) -> String {
        match &self.current_keyspace {
            Some(ks) => format!("cqlsh:{}> ", ks).cyan().to_string(),
            None => "cqlsh> ".cyan().to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("{}", "CoreDB Shell (CQLSH) v0.1.0".bold().green());
    println!("Type 'HELP' for help, 'EXIT' to quit.\n");

    // Initialize database
    let config = DatabaseConfig {
        data_directory: PathBuf::from("./cqlsh_data"),
        commitlog_directory: PathBuf::from("./cqlsh_data/commitlog"),
        ..DatabaseConfig::default()
    };

    let mut shell = CqlShell::new(config).await?;
    let mut rl = DefaultEditor::new()?;

    // Load history if exists
    let history_file = PathBuf::from(".cqlsh_history");
    let _ = rl.load_history(&history_file);

    let mut query_buffer = String::new();

    loop {
        let prompt = if query_buffer.is_empty() {
            shell.get_prompt()
        } else {
            "   ...> ".yellow().to_string()
        };

        let readline = rl.readline(&prompt);
        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str())?;
                
                // Accumulate multi-line queries
                query_buffer.push_str(&line);
                query_buffer.push(' ');
                
                // Check if query is complete (ends with semicolon)
                if line.trim().ends_with(';') || 
                   line.trim().eq_ignore_ascii_case("exit") ||
                   line.trim().eq_ignore_ascii_case("quit") ||
                   line.trim().eq_ignore_ascii_case("help") ||
                   line.trim().to_uppercase().starts_with("DESCRIBE") {
                    
                    // Remove trailing semicolon
                    let query = query_buffer.trim().trim_end_matches(';').to_string();
                    query_buffer.clear();
                    
                    if !query.is_empty() {
                        match shell.execute_line(&query).await {
                            Ok(output) => {
                                if output == "EXIT" {
                                    println!("\n{}", "Goodbye!".green());
                                    break;
                                }
                                println!("{}\n", output);
                            }
                            Err(e) => {
                                println!("{}\n", format!("✗ Error: {}", e).red());
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                query_buffer.clear();
            }
            Err(ReadlineError::Eof) => {
                println!("\n{}", "Goodbye!".green());
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    // Save history
    let _ = rl.save_history(&history_file);

    Ok(())
}
