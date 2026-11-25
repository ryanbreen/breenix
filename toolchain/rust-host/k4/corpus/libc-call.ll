; LLVM Language Reference: call instruction and external declarations.
source_filename = "libc-call.ll"
declare i64 @strlen(ptr)
define i64 @string_length(ptr %s) {
entry:
  %length = call i64 @strlen(ptr %s)
  ret i64 %length
}
