use clap::{Parser, Subcommand};

use opengate::app;
use opengate::mcp;

#[derive(Parser)]
#[command(
    name = "opengate",
    about = "Headless agent-first task management engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the OpenGate engine server
    Serve {
        #[arg(long, default_value = "8080")]
        port: u16,
        #[arg(long, default_value = "opengate.db")]
        db: String,
        #[arg(long, env = "OPENGATE_SETUP_TOKEN", default_value = "")]
        setup_token: String,
    },
    /// Initialize the database
    Init {
        #[arg(long, default_value = "opengate.db")]
        db: String,
    },
    /// Run MCP server (stdio transport) for AI agent integration
    McpServer {
        #[arg(long, default_value = "opengate.db")]
        db: String,
        #[arg(long)]
        agent_key: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            port,
            db,
            setup_token,
        } => {
            app::run_server(port, &db, &setup_token).await;
        }
        Commands::Init { db } => {
            let conn = opengate::db::init_db(&db);
            eprintln!("Database initialized at {}", db);
            drop(conn);
        }
        Commands::McpServer { db, agent_key } => {
            mcp::run_mcp_server(&db, &agent_key).await;
        }
    }
}
