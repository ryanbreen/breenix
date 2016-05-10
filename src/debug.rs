use alloc::boxed::Box;
use buffers::DEBUG_BUFFER;
use collections::vec::Vec;
use core::fmt::Write;
use core::str;
use io::serial;
use x86::msr::{IA32_EFER,TSC,MSR_MCG_RFLAGS};
use x86::msr::rdmsr;
use x86::time::rdtsc;

use memory;
use io::timer;

#[allow(unused_must_use)]
pub fn debug() {

  let mut buffer = DEBUG_BUFFER.lock();
  unsafe {
    let time = timer::time_since_start();
    buffer.write_fmt(format_args!("-------------------------------\n"));
    buffer.write_fmt(format_args!("Time: {}.{}\n", time.secs, time.nanos));
    buffer.write_fmt(format_args!("rdtsc: 0x{:x}\n", rdtsc()));
    buffer.write_fmt(format_args!("msr IA32_EFER: 0x{:x}\n", rdmsr(IA32_EFER)));
    buffer.write_fmt(format_args!("msr TSC: 0x{:x}\n", rdmsr(TSC)));
    buffer.write_fmt(format_args!("msr MSR_MCG_RFLAGS: 0x{:x}\n", rdmsr(MSR_MCG_RFLAGS)));
    buffer.write_fmt(format_args!("interrupt count: 0x20={}, 0x21={}, 0x80={}\n",
      ::state().interrupt_count[0x20], ::state().interrupt_count[0x21], ::state().interrupt_count[0x80]));
    buffer.write_fmt(format_args!("allocated frame count: 0={}\n",
      memory::frame_allocator().allocated_frame_count()));
    buffer.write_fmt(format_args!("{:?}\n", memory::slab_allocator::zone_allocator()));
  }
}

static mut COMMAND_BUFFER:Option<&'static mut Vec<u8>> = None;

pub fn handle_serial_input(c:u8) {

  unsafe {
    match COMMAND_BUFFER {
      None => {
        COMMAND_BUFFER = Some(&mut *Box::into_raw(box vec!()));
        handle_serial_input(c);
      },
      Some(ref mut buf) => {
        if c == 0xD {
          interpret_command(str::from_utf8(buf).unwrap());
          COMMAND_BUFFER = Some(&mut *Box::into_raw(box vec!()));
        } else {
          // Echo to the serial terminal so we aren't typing blind.
          // TODO: Figure out why this requires a newline to flush.
          serial::write_char(c as char);
          serial::write_char('\n');
          buf.push(c);
        }
      }
    }
  }
}

const HELP_COMMAND:&'static str = "help";
const HELP_MNEMONIC:&'static str = "h";
const HELP_DESCRIPTION:&'static str = "Print this help message.";
const DEBUG_COMMAND:&'static str = "debug";
const DEBUG_MNEMOIC:&'static str = "d";
const DEBUG_DESCRIPTION:&'static str = "Dump debug output.";

const COMMANDS:[&'static str;2] = [HELP_COMMAND, DEBUG_COMMAND];
const MNEMONICS:[&'static str;2] = [HELP_MNEMONIC, DEBUG_MNEMOIC];
const DESCRIPTIONS:[&'static str;2] = [HELP_DESCRIPTION, DEBUG_DESCRIPTION];

fn usage() {
  println!("Commands:");
  for i in 0..COMMANDS.len() {
    println!("\t{} ({}) - {}", COMMANDS[i], MNEMONICS[i], DESCRIPTIONS[i]);
  }
}

fn interpret_command(cmd: &'static str) {
  match cmd {
    HELP_COMMAND | HELP_MNEMONIC => {
      usage();
    },
    DEBUG_COMMAND | DEBUG_MNEMOIC => debug(),
    _ => {
      println!("Unknown command {}", cmd);
      usage();
    },
  }
}
