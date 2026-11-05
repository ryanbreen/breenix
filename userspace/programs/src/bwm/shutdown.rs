//! Shutdown decisions at the input/scene wait boundary.

pub trait Runtime {
    type Ready;
    fn unblock(&mut self);
    fn wait(&mut self) -> Self::Ready;
    fn block(&mut self);
    fn terminated(&self) -> bool;
    fn release(&mut self);
    fn restore(&mut self);
    fn exit(&mut self);
}

pub fn boundary<R: Runtime>(runtime: &mut R) -> Option<R::Ready> {
    runtime.unblock();
    let ready = if runtime.terminated() { None } else { Some(runtime.wait()) };
    runtime.block();
    if runtime.terminated() {
        runtime.release();
        runtime.restore();
        runtime.exit();
        None
    } else {
        ready
    }
}
