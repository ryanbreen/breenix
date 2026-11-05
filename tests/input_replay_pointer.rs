extern crate alloc;
#[path = "../kernel/src/drivers/hid_boot/pointer.rs"] mod pointer;
#[test]
fn replay_reports_use_absolute_layout_ranges_ids_padding_and_button_edges() {
    let descriptor=[0x05,1,0x09,2,0xa1,1,0x85,3,0x09,0x30,0x09,0x31,0x15,0,0x26,0xff,0x7f,
        0x75,16,0x95,2,0x81,2,0x05,9,0x09,1,0x15,0,0x25,1,0x75,1,0x95,1,0x81,2,
        0x75,7,0x95,1,0x81,1,0xc0];
    let layout=pointer::Layout::parse(&descriptor).unwrap();
    for (x,y) in [(0,0),(2559,1599),(1234,456)] {
        let reports=layout.replay_reports(x,y,2560,1600).unwrap();
        assert_eq!(reports.len(),2);
        for (i,report) in reports.iter().enumerate() {
            assert_eq!(report[0],3);
            let motion=layout.decode(report).unwrap().unwrap();
            assert_eq!(motion.x.unwrap().position(0,2560),x as u32);
            assert_eq!(motion.y.unwrap().position(0,1600),y as u32);
            assert_eq!(motion.buttons,Some(if i==0 {1} else {0}));
        }
    }
    assert!(layout.replay_reports(-1,0,2560,1600).is_err());
    assert!(layout.replay_reports(2560,0,2560,1600).is_err());
    let mut relative=descriptor; relative[22]=6;
    assert!(pointer::Layout::parse(&relative).unwrap().replay_reports(1,2,2560,1600).is_err());
    let mut caps=Vec::new(); layout.capabilities(&mut |k,c,min,max| caps.push((k,c,min,max)));
    assert!(caps.contains(&(3,0,0,32767)));
}
