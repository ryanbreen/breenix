//! Independent logical oracle; no cowfs codec or writer calls belong here.
use crate::*;

#[derive(Default)]
pub struct Model;
impl Model {
    pub fn new() -> Self {
        Self
    }
}
impl Fs for Model {
    fn execute(&mut self, _op: &Operation) -> Result<Reply> {
        Err(Error::Unimplemented)
    }
    fn state(&self) -> Result<State> {
        Err(Error::Unimplemented)
    }
}

#[cfg(test)]
include!("model_tests.rs");
