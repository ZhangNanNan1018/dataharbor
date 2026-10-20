use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

pub fn block_on<F: Future>(future: F) -> F::Output {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| Runtime::new().expect("tokio runtime"));
    runtime.block_on(future)
}
