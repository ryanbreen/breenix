; LLVM Language Reference: phi, icmp and br instructions.
source_filename = "loop.ll"
define i64 @sum_to(i64 %n) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %body ]
  %sum = phi i64 [ 0, %entry ], [ %added, %body ]
  %done = icmp uge i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %added = add i64 %sum, %i
  %next = add i64 %i, 1
  br label %loop
exit:
  ret i64 %sum
}
