set pagination off
set confirm off
set language c
target remote :1241
echo \n########## SPECIMEN B: stack-word symbolization ##########\n
echo \n===== CPU0 =====\n
thread 1
info registers pc sp cpsr
set $i = 0
while $i < 64
  set $w = *(unsigned long *)($sp + $i*8)
  if $w > 0xffff000040080000 && $w < 0xffff000040800000
    printf "[sp+%#x] %#lx  ", $i*8, $w
    info symbol $w
  end
  set $i = $i + 1
end
echo \n===== CPU1 =====\n
thread 2
info registers pc sp cpsr
set $i = 0
while $i < 64
  set $w = *(unsigned long *)($sp + $i*8)
  if $w > 0xffff000040080000 && $w < 0xffff000040800000
    printf "[sp+%#x] %#lx  ", $i*8, $w
    info symbol $w
  end
  set $i = $i + 1
end
echo \n===== CPU2 =====\n
thread 3
info registers pc sp cpsr
set $i = 0
while $i < 64
  set $w = *(unsigned long *)($sp + $i*8)
  if $w > 0xffff000040080000 && $w < 0xffff000040800000
    printf "[sp+%#x] %#lx  ", $i*8, $w
    info symbol $w
  end
  set $i = $i + 1
end
echo \n===== CPU3 =====\n
thread 4
info registers pc sp cpsr
set $i = 0
while $i < 64
  set $w = *(unsigned long *)($sp + $i*8)
  if $w > 0xffff000040080000 && $w < 0xffff000040800000
    printf "[sp+%#x] %#lx  ", $i*8, $w
    info symbol $w
  end
  set $i = $i + 1
end
detach
quit
