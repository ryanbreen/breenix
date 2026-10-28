//! Backend-qualified completion answers and command kinds.
use gpu_abi::{backend, gk208};
use crate::gpu::{svga, virgl};

pub fn success(number: u32) -> Option<u32> {
    match number {
        backend::VIRGL => Some(virgl::wire::resp::OK_NODATA),
        backend::SVGA3D_DX => Some(svga::wire::status::COMPLETED),
        backend::GK208 => Some(gk208::SUCCESS),
        _ => None,
    }
}
pub fn ok(number: u32, answer: u32) -> bool {
    match number {
        backend::VIRGL => (virgl::wire::resp::OK_NODATA..virgl::wire::resp::ERR_UNSPEC).contains(&answer),
        backend::SVGA3D_DX => answer == svga::wire::status::COMPLETED,
        backend::GK208 => answer == gk208::SUCCESS,
        _ => false,
    }
}
pub fn draw_kind(number: u32) -> Option<u32> {
    match number {
        backend::VIRGL => Some(virgl::wire::cmd::SUBMIT_3D),
        backend::SVGA3D_DX => Some(svga::wire::cmd::DX_DRAW),
        backend::GK208 => Some(gk208::DRAW_RELEASE),
        _ => None,
    }
}
pub fn upload_kind(number: u32) -> Option<u32> {
    match number {
        backend::VIRGL => Some(virgl::wire::cmd::TRANSFER_TO_HOST_3D),
        backend::SVGA3D_DX => Some(svga::wire::cmd::UPDATE_SURFACE),
        backend::GK208 => Some(gk208::TRANSFER_RELEASE),
        _ => None,
    }
}
