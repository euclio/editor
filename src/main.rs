use std::error::Error;

use tokio::runtime::Builder;

use editor::Options;
use structopt::StructOpt;

fn main() -> Result<(), Box<dyn Error>> {
    editor::Logger::init("RUST_LOG", "/tmp/editor.log");

    let options = Options::from_args();

    let runtime = Builder::new_current_thread().enable_io().build()?;
    runtime.block_on(editor::run(options))?;

    Ok(())
}
