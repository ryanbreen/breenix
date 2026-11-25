; LLVM Language Reference: structure types and getelementptr.
source_filename = "struct.ll"
%pair = type { i32, i64 }
define i64 @pair_sum(ptr %p) {
entry:
  %first = getelementptr %pair, ptr %p, i64 0, i32 0
  %second = getelementptr %pair, ptr %p, i64 0, i32 1
  %a = load i32, ptr %first
  %b = load i64, ptr %second
  %wide = sext i32 %a to i64
  %sum = add i64 %wide, %b
  ret i64 %sum
}
