set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file /Users/wrb/fun/code/breenix/turn2-artifacts/parallels-gdb-breakpoints/breakpoint-count.log
set logging overwrite on
set logging enabled on
target remote 127.0.0.1:9612
set $poll_hits = 0
set $irq_hits = 0
break *0xffff0000400b572c
commands
silent
set $poll_hits = $poll_hits + 1
printf "POLL_HIT %d pc=%p\n", $poll_hits, $pc
continue
end
break *0xffff0000400b4cbc
commands
silent
set $irq_hits = $irq_hits + 1
printf "IRQ_HIT %d pc=%p\n", $irq_hits, $pc
continue
end
printf "BREAKPOINTS_ARMED\n"
continue
