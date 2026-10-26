//! Native boot attachment resizes the retained console grid.
extern crate alloc;
#[path = "../kernel/src/console/boxdraw.rs"] pub mod boxdraw;
#[path = "../kernel/src/console/grid.rs"] pub mod grid;
#[path = "../kernel/src/console/raster.rs"] pub mod raster;
use grid::{Grid,BLANK};
fn text(g:&Grid,row:usize)->String {
    g.row(row).iter().map(|c|c.character()).collect::<String>().trim_end().to_string()
}
#[test]
    fn native_boot_resize_preserves_text_history_cursor_and_pen() {
        let mut g=Grid::new();
        g.set_storage(vec![BLANK;8*6].leak(),vec![BLANK;8*6].leak());
        g.set_size(4,2);
        g.set_history(vec![BLANK;grid::HISTORY_LINES*8].leak());
        g.write_kernel(b"one\ntwo\n",false);
        g.write(b"\x1b[31mR");
        let red=g.row(1)[0];
        assert_eq!(text(&g,0),"two");
        assert_eq!(g.cursor(),(1,1));
        g.resize_boot(3,4).unwrap();
        assert_eq!((g.cols(),g.rows()),(3,4));
        assert_eq!(text(&g,0),"two");
        assert_eq!(text(&g,1),"R");
        assert_eq!(g.row(1)[0],red);
        assert_eq!(text(&g,2),"");
        assert_eq!(g.cursor(),(1,1));
        g.scroll_view(isize::MAX);
        assert_eq!(g.view_row(0).iter().map(|c|c.character()).collect::<String>(),"one");
        g.view_live();
        g.write(b"S");
        assert_eq!(text(&g,1),"RS");
        assert_eq!(g.row(1)[1].colours,red.colours);
        assert!(g.resize_boot(9,9).is_err());
        assert_eq!((g.cols(),g.rows()),(3,4));
        assert_eq!(text(&g,1),"RS");
    }

