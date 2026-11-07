//! Fixed stage approval inputs, compiled only by the BreenGL suite.
use libgfx::{color::Color,surface::{Format,Surface}};
use libgfx_ref::stage as oracle;

/// U14b approval rendering. The live case has already compared its own frame exactly.
/// Draw the documented fixed run with the same GPU painter, and compare its full readback
/// against the shared stage reference before handing either picture to the runner.
pub(crate) fn render(case: &str, width: usize, height: usize, boundary: oracle::Observation)
    -> Result<(Surface, Vec<u8>, Vec<u8>), String>
{
    use crate::console_picture as fixed;
    fixed::with_stage(case,boundary,fixed::FIXED_STAGE_MS,fixed::FIXED_STAGE_KEY,|run| {
        let mut reference=libgfx_ref::framebuf::FrameBuf::new(width,height,libgfx_ref::framebuf::Format::BGRX)
            .map_err(|e|format!("fixed stage reference: {e:?}"))?;
        oracle::draw(&mut reference,run).map_err(|e|format!("fixed stage reference: {e:?}"))?;
        let tiles=[libgfx::stage::Tile{id:case,stamp:libgfx::stage::Stamp::Pass,key:run.tiles[0].key,thumbnail:None}];
        let milestones=[libgfx::stage::Milestone{id:run.milestones[0].id,first:0,end:1}];
        let model=libgfx::stage::Run{suite:run.suite,target:run.target,case_number:run.case_number,
            ui_time_ms:run.ui_time_ms,fps:run.fps,tiles:&tiles,milestones:&milestones,selected:run.selected,
            input:None,spotlight:libgfx::stage::Spotlight::None,finale:None};
        let mut canvas=Surface::new(width,height,Format::BGRX).map_err(|e|format!("fixed stage canvas: {e}"))?;
        let mut stage=libgfx::stage::Stage::new();
        stage.draw(&mut canvas,&model).map_err(|e|format!("fixed stage paint: {e:?}"))?;
        for (x,y) in fixed::cursor_positions(case) {
            let (outline,fill)=libbreenix::compositor::sprite(libbreenix::window::abi::cursor_shape::ARROW,x,y);
            for (rectangles,c) in [(outline,[0,0,0]),(fill,[255,255,255])] {
                for [x0,y0,x1,y1] in rectangles {
                    libgfx::shapes::fill_rect(&mut canvas,x0,y0,x1-x0,y1-y0,Color::rgb(c[0],c[1],c[2]));
                    libgfx_ref::shapes::fill_rect(&mut reference,x0,y0,x1-x0,y1-y0,libgfx_ref::color::Color::rgb(c[0],c[1],c[2]));
                }
            }
        }
        canvas.wait().map_err(|e|format!("fixed stage render: {e}"))?;
        let mut actual=vec![0;width*height*4];
        canvas.read(&mut actual,width*4).map_err(|e|format!("fixed stage readback: {e}"))?;
        let mut expected=Vec::with_capacity(actual.len());
        for y in 0..height {for x in 0..width {expected.extend_from_slice(&reference.get_bgra(x,y));}}
        for p in actual.chunks_exact_mut(4) {p[3]=255;}
        if let Some(i)=actual.iter().zip(&expected).position(|(a,b)|a!=b) {
            return Err(format!("fixed stage differs from renderer inputs at pixel {},{} channel {}",(i/4)%width,(i/4)/width,i%4));
        }
        Ok((canvas,actual,expected))
    })?
}
