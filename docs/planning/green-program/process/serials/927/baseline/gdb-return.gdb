set pagination off
set confirm off
set language c
target remote 127.0.0.1:1927
printf "CORRECTION: Result<ProcessId, &str> uses a pointer niche, no separate discriminant. First word is Err pointer, second word is length 45.\n"
p *(char (*)[45])0x10000090f33
x/3gx 0xffffc900000ff7e0
detach
quit
