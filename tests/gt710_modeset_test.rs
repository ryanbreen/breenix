//! Native timings and physical DMA objects are encoded independently of POST.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path = "../kernel/src/gpu/gk208/modeset.rs"] mod modeset;
use edid::Timing;
fn preferred() -> Timing {
    let mut raw=[0u8;128];
    raw[..8].copy_from_slice(&[0,255,255,255,255,255,255,0]);
    raw[18..21].copy_from_slice(&[1,3,128]);
    raw[54..72].copy_from_slice(&[0x30,0x2a,0,0x98,0x51,0,0x2a,0x40,0x30,0x70,0x13,0,0,0,0,0,0,0x1e]);
    raw[127]=0u8.wrapping_sub(raw.iter().fold(0u8,|a,b|a.wrapping_add(*b)));
    edid::parse(&raw).unwrap()[0]
}
#[test]
fn encodes_preferred_timing_and_owned_pitch_surface() {
    let words=modeset::head_methods(&preferred(),0,0,1,0x08010000,5120).unwrap();
    let m:std::collections::BTreeMap<_,_>=words.into_iter().collect();
    assert_eq!(m[&0x414],1066<<16|1688);
    assert_eq!(m[&0x418],2<<16|111);
    assert_eq!(m[&0x41c],40<<16|359);
    assert_eq!(m[&0x420],1064<<16|1639);
    assert_eq!(m[&0x450],108000000);
    assert_eq!(m[&0x460],0x080100);
    assert_eq!(m[&0x468],1024<<16|1280);
    // NV927D_HEAD_SET_STORAGE_PITCH is bits 20:8, in 256-byte units.
    assert_eq!(m[&0x46c],0x01001400);
    assert_eq!(modeset::head_methods(&preferred(),0,0,1,0x08010000,8191*256)
        .unwrap().into_iter().find(|(r,_)|*r==0x46c).unwrap().1,0x011fff00);
    assert!(modeset::head_methods(&preferred(),0,0,1,0x08010000,8192*256).is_err());
    assert_eq!(m[&0x470],0xcf00);
    assert_eq!(m[&0x474],1);
    assert_eq!(m[&0x498],0x40000);
    assert_eq!(m[&0x4a0],0);
    assert_eq!(m[&0x200],0x101);
    assert_eq!(m[&0x404]&0x3d8,0x140);
    for (head,sor,address,pitch) in [(4,0,0x08010000,5120),(0,4,0x08010000,5120),
        (0,0,0x08010001,5120),(0,0,0x08010000,5119)] {
        assert!(modeset::head_methods(&preferred(),head,sor,1,address,pitch).is_err());
    }
    let mut bad=preferred();bad.hsync_end=bad.htotal+1;
    assert!(modeset::head_methods(&bad,0,0,1,0x08010000,5120).is_err());
}
#[test]
fn physical_dma_and_ramht_entries_stay_within_private_instance() {
    assert_eq!(modeset::dma_object(0x08010000,0x500000).unwrap(),
        [0x49,0x080100,0x0850ff,0,0,0]);
    assert!(modeset::dma_object(3,4096).is_err());
    assert!(modeset::dma_object(1<<40,4096).is_err());
    assert_eq!(modeset::ramht_slot(1,0),8);
    assert_eq!(modeset::ramht_slot(2,0),16);
    assert_eq!(modeset::ramht_slot(0x201,0),0);
}
#[test]
fn chooses_a_bounded_native_pixel_clock_from_board_limits() {
    let limits=modeset::PllLimits { reg:0x614100,reference:27000,min_vco:500000,max_vco:1000000,
        min_input:1000,max_input:27000,min_m:1,max_m:255,min_n:1,max_n:255,min_p:1,max_p:63 };
    let p=modeset::pll(&limits,108000).unwrap();
    let actual=27000.0*(p.n as f64+0.5+(p.fraction as i16 as f64)/8192.0)/(p.m as f64*p.p as f64);
    assert!((actual-108000.0).abs()<10.0);
    assert!(p.p>0 && p.m>0);
    assert!(modeset::pll(&limits,0).is_err());
    let mut bad=limits;bad.min_m=0;
    assert!(modeset::pll(&bad,108000).is_err());
}

#[test]
fn parses_only_the_selected_board_pixel_pll_with_bounded_limits() {
    let mut rom=vec![0u8;1024];
    rom[0x80..0x8c].copy_from_slice(&[255,184,66,73,84,0,0,1,12,6,1,0]);
    rom[0x8c..0x92].copy_from_slice(&[b'C',2,4,0,0xa0,0]);
    rom[0xa0..0xa4].copy_from_slice(&0xb0u32.to_le_bytes());
    rom[0xb0..0xb4].copy_from_slice(&[0x40,4,12,1]);
    rom[0xb4]=0x80;
    rom[0xb5..0xb7].copy_from_slice(&0x100u16.to_le_bytes());
    rom[0xb7..0xbb].copy_from_slice(&0x614100u32.to_le_bytes());
    rom[0xbd..0xbf].copy_from_slice(&27u16.to_le_bytes());
    for (i,value) in [500u16,1000,1,27].into_iter().enumerate() {
        rom[0x100+i*2..0x102+i*2].copy_from_slice(&value.to_le_bytes());
    }
    rom[0x108..0x10e].copy_from_slice(&[1,255,1,255,1,63]);
    let l=modeset::board_pll(&rom,0).unwrap();
    assert_eq!((l.reg,l.reference,l.min_vco,l.max_vco),(0x614100,27000,500000,1000000));
    assert!(modeset::board_pll(&rom,1).is_err());
    assert!(modeset::board_pll(&rom,4).is_err());
    for n in [0,64,0x90,0xb5,0xbf,0x108,0x10d] {
        assert!(modeset::board_pll(&rom[..n],0).is_err());
    }
    rom[0x108]=0;
    assert!(modeset::pll(&modeset::board_pll(&rom,0).unwrap(),108000).is_err());
}
#[test]
fn quiesce_refuses_a_busy_channel_before_replacing_display_storage() {
    struct Regs { values:std::collections::BTreeMap<usize,u32>,busy:bool,writes:Vec<usize> }
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {
            if self.busy && r==0x610490 {0x001e0000} else {*self.values.get(&r).unwrap_or(&0)}
        }
        fn write(&mut self,r:usize,v:u32) { self.writes.push(r);self.values.insert(r,v); }
        fn delay_us(&mut self,_:u32) {}
    }
    for busy in [false,true] {
        let mut r=Regs{values:Default::default(),busy,writes:vec![]};
        assert_eq!(modeset::quiesce(&mut r).is_err(),busy);
        assert!(r.writes.iter().all(|r|(0x610490..0x6105a0).contains(r)),"quiesce wrote outside channel controls");
        if !busy {for channel in 0..17 {assert!(r.writes.contains(&(0x610490+channel*16)));}}
    }
}
#[test]
fn board_scripts_are_predecoded_and_bounded_before_display_writes() {
    let mut rom=vec![0;128];
    rom[16..26].copy_from_slice(&[0x7a,0x00,0x10,0x61,0x80,0x78,0x56,0x34,0x12,0x71]);
    let ops=modeset::script(&rom,16,1,0,1).unwrap();
    assert_eq!(ops,[modeset::Op::Register(0x611800,0,0x12345678)]);
    rom[16]=0xff;
    let refusal=modeset::script(&rom,16,1,0,1).unwrap_err();
    assert_eq!((refusal.offset,refusal.opcode),(16,0xff));
    assert_eq!(refusal.reason,"unsupported display script opcode");
    rom[16..19].copy_from_slice(&[0x5b,16,0]);
    assert!(modeset::script(&rom,16,1,0,1).is_err(),"recursive script escaped bounds");
    assert!(modeset::script(&rom[..17],16,1,0,1).is_err());
    rom[16..26].copy_from_slice(&[0x7a,0,0,0x70,0,0,0,0,0,0x71]);
    assert!(modeset::script(&rom,16,1,0,1).is_err(),"script wrote inherited PRAMIN storage");
}
#[test]
fn selects_the_board_iedt_for_the_connected_head_and_tmds_link() {
    let mut rom=vec![0u8;512];
    rom[0x80..0x8c].copy_from_slice(&[255,184,66,73,84,0,0,1,12,6,1,0]);
    rom[0x8c..0x92].copy_from_slice(&[b'U',1,2,0,0xa0,0]);
    rom[0xa0..0xa2].copy_from_slice(&0x100u16.to_le_bytes());
    rom[0x100..0x107].copy_from_slice(&[0x22,5,2,1,12,0x20,1]);
    rom[0x120..0x126].copy_from_slice(&[2,0,0x41,1,0,0]);
    rom[0x128..0x12a].copy_from_slice(&0x180u16.to_le_bytes());
    rom[0x180..0x18a].copy_from_slice(&[0x7a,0,0x10,0x61,0x80,1,0,0,0,0x71]);
    let ops=modeset::board_scripts(&rom,0,0,1,108000).unwrap();
    assert_eq!(ops[0],[modeset::Op::Register(0x611000,0,1)]);
    assert!(ops[1..].iter().all(Vec::is_empty));
    assert!(modeset::board_scripts(&rom,1,0,1,108000).is_err());
    assert!(modeset::board_scripts(&rom,0,0,2,108000).is_err());
    assert!(modeset::board_scripts(&rom[..0x185],0,0,1,108000).is_err());
    // OCFG flags are u8; the next byte is the low byte of a script-table pointer.
    rom[0x125]=1;
    rom[0x12c..0x132].copy_from_slice(&[1,0,0x91,1,0,0]);
    rom[0x191..0x195].copy_from_slice(&[0x30,0x2a,0xa0,1]);
    rom[0x1a0..0x1aa].copy_from_slice(&[0x7a,0,0x23,0x61,0,2,0,0,0,0x71]);
    let selected=modeset::board_scripts(&rom,0,0,1,108000).unwrap();
    assert_eq!(selected[2],[modeset::Op::Register(0x612300,0,2)]);
}
#[test]
fn copy_nv_reg_preserves_destination_bits_and_handles_both_shift_directions() {
    struct Regs {src:u32,dst:u32}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {match r {0x611800=>self.src,0x61c800=>self.dst,_=>panic!("unexpected read {r:x}")}}
        fn write(&mut self,r:usize,v:u32) {assert_eq!(r,0x61c800);self.dst=v;}
        fn delay_us(&mut self,_:u32) {panic!("unexpected delay");}
    }
    for (shift,source,mask,xor,keep,expected) in [
        (8,0x1234ab00,0xff,0x55,0xffffff00,0xdeadbefe),
        (248,0x12,0xffff,0x55,0xffff0000,0xdead1255)] {
        let mut rom=vec![0;64];let p=16;
        rom[p]=0x5f;
        rom[p+1..p+5].copy_from_slice(&0x80611000u32.to_le_bytes());
        rom[p+5]=shift;
        rom[p+6..p+10].copy_from_slice(&u32::to_le_bytes(mask));
        rom[p+10..p+14].copy_from_slice(&u32::to_le_bytes(xor));
        rom[p+14..p+18].copy_from_slice(&0x4061c000u32.to_le_bytes());
        rom[p+18..p+22].copy_from_slice(&u32::to_le_bytes(keep));
        rom[p+22]=0x71;
        let ops=modeset::script(&rom,p,1,1,1).unwrap();
        let mut r=Regs{src:source,dst:0xdeadbeef};
        modeset::execute_ops(&mut r,&ops);
        assert_eq!(r.dst,expected);
        for invalid in [32,128,224] {
            rom[p+5]=invalid;
            assert!(modeset::script(&rom,p,1,1,1).is_err());
        }
        rom[p+5]=shift;
        assert!(modeset::script(&rom[..p+21],p,1,1,1).is_err());
    }
}
#[test]
fn evo_publication_requires_the_device_bar_flush_to_finish() {
    struct Regs {busy:bool,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {assert_eq!(r,0x70000);if self.busy {2} else {0}}
        fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));}
        fn delay_us(&mut self,_:u32) {}
    }
    for busy in [false,true] {
        let mut r=Regs{busy,writes:vec![]};
        assert_eq!(modeset::publish(&mut r,128).is_err(),busy);
        assert_eq!(r.writes[0],(0x70000,1));
        assert_eq!(&r.writes[1..],if busy {&[][..]} else {&[(0x640000,128)][..]});
    }
}

#[test]
fn timed_board_condition_bounds_polling_and_preserves_resume_and_call_semantics() {
    struct Regs {ready_after:usize,reads:usize,delays:usize,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {
            if r==0x611800 {self.reads+=1;u32::from(self.reads>self.ready_after)} else {0}
        }
        fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));}
        fn delay_us(&mut self,us:u32) {assert_eq!(us,20000);self.delays+=1;}
    }
    let mut rom=vec![0;1024];
    rom[0x80..0x8c].copy_from_slice(&[255,184,66,73,84,0,0,1,12,6,1,0]);
    rom[0x8c..0x92].copy_from_slice(&[b'I',1,8,0,0xa0,0]);
    rom[0xa6..0xa8].copy_from_slice(&0x200u16.to_le_bytes());
    let c=0x200+5*12;
    for (i,v) in [0x80611000u32,1,1].into_iter().enumerate() {
        rom[c+i*4..c+i*4+4].copy_from_slice(&v.to_le_bytes());
    }
    // A skipped call must not execute its RESUME; the caller's RESUME restores writes.
    rom[16..19].copy_from_slice(&[0x56,5,255]);
    rom[19..28].copy_from_slice(&[0x7a,0,0,0x61,0,1,0,0,0]);
    rom[28..31].copy_from_slice(&[0x5b,0,1]);
    rom[31..42].copy_from_slice(&[0x72,0x7a,4,0,0x61,0,3,0,0,0,0x71]);
    rom[0x100..0x10b].copy_from_slice(&[0x72,0x7a,8,0,0x61,0,2,0,0,0,0x71]);
    let ops=modeset::script(&rom,16,1,0,1).unwrap();
    for (ready_after,reads,delays,writes) in [
        (2,3,2,vec![(0x610000,1),(0x610008,2),(0x610004,3)]),
        (usize::MAX,100,100,vec![(0x610004,3)])] {
        let mut r=Regs{ready_after,reads:0,delays:0,writes:vec![]};
        modeset::execute_ops(&mut r,&ops);
        assert_eq!((r.reads,r.delays,r.writes),(reads,delays,writes));
    }
    let original=rom.clone();
    rom[c..c+4].copy_from_slice(&0x700000u32.to_le_bytes());
    assert!(modeset::script(&rom,16,1,0,1).is_err(),"condition escaped display MMIO");
    rom=original.clone();rom[0xa6..0xa8].fill(0);
    assert!(modeset::script(&rom,16,1,0,1).is_err(),"missing condition table accepted");
    rom=original;rom[0x100..0x104].copy_from_slice(&[0x56,5,255,0x71]);
    assert!(modeset::script(&rom,16,1,0,1).is_err(),"nested condition polling escaped total bound");
}

#[test]
fn quiesce_stops_satellite_channels_before_the_core() {
    struct Regs {control:[u32;17]}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {
            let i=(r-0x610490)/16;
            let blocked=i==0 && self.control[1..].iter().any(|v|v&1!=0);
            self.control[i] | if blocked {0x40000} else {0}
        }
        fn write(&mut self,r:usize,v:u32) {self.control[(r-0x610490)/16]=v;}
        fn delay_us(&mut self,_:u32) {}
    }
    let mut r=Regs{control:[1;17]};
    modeset::quiesce(&mut r).unwrap();
    assert!(r.control.iter().all(|v|v&1==0));
}

#[test]
fn inherited_requests_must_read_back_disabled_before_instance_binding() {
    struct Regs {control:[u32;17],stuck:Option<usize>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {self.control[(r-0x610490)/16]}
        fn write(&mut self,r:usize,v:u32) {
            let i=(r-0x610490)/16;
            if self.stuck!=Some(i) {self.control[i]=v;}
        }
        fn delay_us(&mut self,_:u32) {}
    }
    for stuck in [None,Some(0),Some(1),Some(9),Some(16)] {
        let mut r=Regs{control:core::array::from_fn(|i|if i<9 {0x40000013} else {0x85}),stuck};
        assert_eq!(modeset::stop_requests(&mut r).is_err(),stuck.is_some());
        if stuck.is_none() {
            for (i,v) in r.control.iter().enumerate() {
                assert_eq!(v & if i<9 {0x13} else {1},0);
                assert_eq!(v & if i<9 {0x40000000} else {0x84},if i<9 {0x40000000} else {0x84});
            }
        }
    }
}

#[test]
fn preparing_the_disabled_core_requires_empty_cursors_and_disabled_requests() {
    struct Regs {control:u32,put:u32,get:u32,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {match r {0x610490=>self.control,0x640000=>self.put,0x640004=>self.get,_=>panic!("unexpected read")}}
        fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));if r==0x610490 {self.control=v;}}
        fn delay_us(&mut self,_:u32) {}
    }
    for (control,put,get,push,ok) in [(0xa8,0,0,0x08010000,true),
        (0xa9,0,0,0x08010000,false),(0xba,0,0,0x08010000,false),
        (0xb8,0,0,0x08010000,false),(0xa8,0,0,0,false),
        (0xa8,4,0,0x08010000,false),(0xa8,0,4,0x08010000,false),
        (0xa8,0,0,0x08010001,false),(0xa8,0,0,1<<40,false)] {
        let mut r=Regs{control,put,get,writes:vec![]};
        assert_eq!(modeset::prepare_disabled_core(&mut r,push).is_ok(),ok);
        if ok {
            assert!(r.writes.contains(&(0x610494,0x080101)));
            assert!(r.writes.iter().filter(|(reg,_)|*reg==0x610490).all(|(_,v)|v&3==0));
            assert_eq!(r.control&0x13,0x10,"preparation enabled a channel request");
        } else {assert!(r.writes.is_empty(),"unsafe core preparation performed writes");}
    }
}

#[test]
fn empty_core_bootstrap_requires_owned_bindings_and_waits_for_initialization() {
    struct Regs {control:u32,instance:u32,push:u32,get:u32,busy:bool,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {match r {0x610490=>self.control|if self.busy {0x80000000} else {0},
            0x610010=>self.instance,0x610494=>self.push,0x640000=>0,0x640004=>self.get,_=>panic!("unexpected read")}}
        fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));if r==0x610490 {self.control=v;}}
        fn delay_us(&mut self,_:u32) {}
    }
    for (control,instance,push,get,busy,allowed) in [
        (0xb8,0x81009,0x80101,0,false,true),(0xb8,0x81009,0x80101,0,true,true),
        (0xb9,0x81009,0x80101,0,false,false),(0xa8,0x81009,0x80101,0,false,false),
        (0xb8,1,0x80101,0,false,false),(0xb8,0x81009,1,0,false,false),
        (0xb8,0x81009,0x80101,4,false,false)] {
        let mut r=Regs{control,instance,push,get,busy,writes:vec![]};
        assert_eq!(modeset::initialize_empty_core(&mut r,0x08100000,0x08010000).is_ok(),allowed&&!busy);
        assert_eq!(r.writes,if allowed {vec![(0x610490,0x01000013)]} else {vec![]});
    }
}

#[test]
fn bar1_switch_waits_for_both_device_completion_drains() {
    struct Regs {requests:usize,blocked:usize}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {assert_eq!(r,0x70000);if self.requests==self.blocked {2} else {0}}
        fn write(&mut self,r:usize,v:u32) {assert_eq!((r,v),(0x70000,1));self.requests+=1;}
        fn delay_us(&mut self,_:u32) {}
    }
    for blocked in [0,1,2] {
        let mut r=Regs{requests:0,blocked};
        assert_eq!(modeset::bar1_wait(&mut r).is_err(),blocked!=0);
        assert_eq!(r.requests,if blocked==1 {1} else {2});
    }
}

#[test]
fn native_bar_view_refuses_a_scanout_beyond_the_bounded_cpu_mapping() {
    assert_eq!(modeset::bar1_view(0x08010000,0x500000,0x8000000,16).unwrap(),(0x08010000,0,80));
    assert_eq!(modeset::bar1_view(0x08010000,0x500000,0x8000000,17).unwrap(),(0x08000000,65536,41));
    for (start,bytes,aperture,page) in [(0x08010000,0x800000,0x8000000,17),
        (0x08010000,1920*4*1200,0x8000000,16),(0x08010000,0x1000,0x400000,16),
        (0x08010000,0,0x8000000,16),(u64::MAX-4095,8192,0x8000000,16),
        (1<<40,4096,0x8000000,16),(0x08010000,4096,0x8000000,12)] {
        assert!(modeset::bar1_view(start,bytes,aperture,page).is_err());
    }
}

#[test]
fn bar1_page_setup_requires_inactive_vm_and_selects_128k() {
    struct Regs {bar1:u32,bar2:u32,config:u32,stuck:bool,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {match r {0x1704=>self.bar1,0x1714=>self.bar2,0x100c80=>self.config,_=>panic!("unexpected read")}}
        fn write(&mut self,r:usize,v:u32) {assert_eq!(r,0x100c80);self.writes.push((r,v));if !self.stuck {self.config=v;}}
        fn delay_us(&mut self,_:u32) {}
    }
    for (bar1,bar2,config,stuck,ok) in [
        (0x7ff00,0x40000000,0x208001,false,true),
        (0x7ff00,0x40000000,0x208000,false,true),
        (0x8007ff00,0x40000000,0x208001,false,false),
        (0x7ff00,0xc0000000,0x208001,false,false),
        (0x7ff00,0x40000000,0x208001,true,false)] {
        let mut r=Regs {bar1,bar2,config,stuck,writes:vec![]};
        assert_eq!(modeset::bar1_page(&mut r),if ok {Ok(17)} else {Err("BAR page setup requires inactive VM and a latched 128 KiB page size")});
        assert_eq!(r.writes,if bar1&0x80000000!=0 || bar2&0x80000000!=0 {vec![]} else {vec![(0x100c80,config&!1)]});
    }
}

#[test]
fn bar1_activation_finishes_switch_drains_before_invalidating_the_mapping() {
    struct Regs {drains:usize,blocked:usize,free:bool,queued:bool,writes:Vec<(usize,u32)>}
    impl modeset::Io for Regs {
        fn read(&mut self,r:usize)->u32 {match r {
            0x1704=>0x7ff00,
            0x70000=>if self.drains==self.blocked {2} else {0},
            0x100c80=>(if self.free {0x200000} else {0})|(if self.queued {0x8000} else {0}),
            _=>panic!("unexpected read {r:x}")}}
        fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));if r==0x70000 {self.drains+=1;}}
        fn delay_us(&mut self,_:u32) {}
    }
    let sequence=[(0x70000,1),(0x1704,0x80008a32),(0x70000,1),(0x70000,1),
        (0x70000,1),(0x100cb8,0x8a330),(0x100cbc,0x80000005)];
    for (blocked,free,queued) in [(0,true,true),(1,true,true),(2,true,true),
        (3,true,true),(4,true,true),(0,false,true),(0,true,false)] {
        let mut r=Regs{drains:0,blocked,free,queued,writes:vec![]};
        let result=modeset::activate_bar1(&mut r,0x8a32000,0x8a33000);
        assert_eq!(result.is_ok(),blocked==0&&free&&queued);
        if result.is_ok() {assert_eq!(result.unwrap(),0x7ff00);}
        let length=if blocked>0 {if blocked==1 {1} else {blocked+1}} else if !free {5} else {7};
        assert_eq!(r.writes,sequence[..length]);
    }
}

#[test]
fn bar1_small_pages_use_the_spt_half_and_bound_each_linear_page() {
    let (pde,ptes)=modeset::bar1_small_pages(0x08000000,0x520000,0x08a34000).unwrap();
    assert_eq!(pde,[0,0x8a341]);
    assert_eq!(ptes.len(),2048);
    assert_eq!(ptes[0],0x80001);
    assert_eq!(ptes[1],0x80011);
    assert_eq!(ptes[16],0x80101);
    assert_eq!(ptes[1311],0x851f1);
    assert!(ptes[1312..].iter().all(|pte|*pte==0));
    for (base,span,table) in [(0x08000001,0x520000,0x08a34000),
        (0x08000000,0,0x08a34000),(0x08000000,0x801000,0x08a34000),
        (0x08000000,0x520001,0x08a34000),(0x08000000,0x520000,0x08a34001),
        ((1<<40)-4096,8192,0x08a34000),(0x08000000,4096,1<<40)] {
        assert!(modeset::bar1_small_pages(base,span,table).is_err());
    }
}
