use tokio::runtime::Builder;

use editor::Options;
use structopt::StructOpt;

fn main() {
    editor::Logger::init("RUST_LOG", "/tmp/editor.log");

    let options = Options::from_args();

    let mut runtime = Builder::new()
        .basic_scheduler()
        .enable_io()
        .build()
        .unwrap();

    runtime.block_on(editor::run(options)).unwrap();
}
