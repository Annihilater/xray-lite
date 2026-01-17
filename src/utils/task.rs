use std::cell::RefCell;
use std::future::Future;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RuntimeMode {
    Tokio,
    Monoio,
}

thread_local! {
    static RUNTIME_MODE: RefCell<RuntimeMode> = RefCell::new(RuntimeMode::Tokio);
}

pub fn set_runtime_mode(mode: RuntimeMode) {
    RUNTIME_MODE.with(|m| *m.borrow_mut() = mode);
}

pub fn get_runtime_mode() -> RuntimeMode {
    RUNTIME_MODE.with(|m| *m.borrow())
}

/// A unified spawn function that adapts to the current runtime.
/// Note: This spawns disjoint tasks (fire-and-forget).
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    // We assume the future is !Send, so we use spawn_local for Tokio
    // and monoio::spawn for Monoio.
    RUNTIME_MODE.with(|mode| match *mode.borrow() {
        RuntimeMode::Tokio => {
            tokio::task::spawn_local(future);
        }
        RuntimeMode::Monoio => {
            monoio::spawn(future);
        }
    });
}
