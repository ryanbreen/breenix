set pagination off
set confirm off
set print pretty off
set print elements 400
target remote :1240
echo \n########## SPECIMEN: aarch64 production-profile wedge, 4 vCPUs ##########\n
echo \n===== info threads (QEMU vCPUs) =====\n
info threads
echo \n===== CPU0 =====\n
thread 1
info registers pc sp cpsr
p/x $TPIDR_EL1
bt 14
echo \n===== CPU1 =====\n
thread 2
info registers pc sp cpsr
p/x $TPIDR_EL1
bt 14
echo \n===== CPU2 =====\n
thread 3
info registers pc sp cpsr
p/x $TPIDR_EL1
bt 17
echo \n===== CPU3 =====\n
thread 4
info registers pc sp cpsr
p/x $TPIDR_EL1
bt 14
echo \n===== SCHEDULER spin lock byte (1 = HELD) =====\n
p kernel::task::scheduler::SCHEDULER.inner.lock
echo \n===== interrupted contexts =====\n
thread 1
frame 9
echo \n--- CPU0 interrupted context (elr / spsr / x30) ---\n
p/x (*frame).elr
p/x (*frame).spsr
p/x (*frame).x30
info symbol (*frame).elr
info symbol (*frame).x30
thread 2
frame 9
echo \n--- CPU1 interrupted context (elr / spsr / x30) ---\n
p/x (*frame).elr
p/x (*frame).spsr
p/x (*frame).x30
info symbol (*frame).elr
info symbol (*frame).x30
thread 4
frame 9
echo \n--- CPU3 interrupted context (elr / spsr / x30) ---\n
p/x (*frame).elr
p/x (*frame).spsr
p/x (*frame).x30
info symbol (*frame).elr
info symbol (*frame).x30
echo \n===== per-CPU scheduler state =====\n
p kernel::task::scheduler::SCHEDULER.inner.data.value.0.cpu_state
echo \n===== per-CPU ready queues =====\n
p kernel::task::scheduler::SCHEDULER.inner.data.value.0.per_cpu_queues[0]
p kernel::task::scheduler::SCHEDULER.inner.data.value.0.per_cpu_queues[1]
p kernel::task::scheduler::SCHEDULER.inner.data.value.0.per_cpu_queues[2]
p kernel::task::scheduler::SCHEDULER.inner.data.value.0.per_cpu_queues[3]
echo \n===== thread table =====\n
set $n = kernel::task::scheduler::SCHEDULER.inner.data.value.0.threads.len
set $b = kernel::task::scheduler::SCHEDULER.inner.data.value.0.threads.buf.inner.ptr.pointer.pointer
printf "thread count = %lu\n", $n
set $i = 0
while $i < $n
  set $tp = *(($b + $i * 8) as *mut u64)
  set $t = $tp as *mut kernel::task::thread::Thread
  printf "tid=%-4lu ", (*$t).id
  printf "state="
  output (*$t).state
  printf " priv="
  output (*$t).privilege
  printf " owner_pid="
  output (*$t).owner_pid
  printf " blocked_in_syscall="
  output (*$t).blocked_in_syscall
  printf " affinity="
  output (*$t).cpu_affinity
  printf " has_started="
  output (*$t).has_started
  printf " name="
  eval "x/%dc (*$t).name.vec.buf.inner.ptr.pointer.pointer", (*$t).name.vec.len
  set $i = $i + 1
end
echo \n===== branch counters =====\n
p kernel::task::idle_sleep::IDLE_SLEEP_REFUSED
p kernel::task::idle_sleep::IDLE_IDENTITY_UNREADABLE
p kernel::task::scheduler::PINNED_HOME_CPU_UNAVAILABLE
detach
quit
