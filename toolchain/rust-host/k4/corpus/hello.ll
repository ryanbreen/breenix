; LLVM Language Reference: global constants and call instructions.
source_filename = "hello.ll"
@line = private constant [16 x i8] c"Hello from llc!\00"
declare i32 @puts(ptr)
define i32 @main() {
entry:
  %ignored = call i32 @puts(ptr @line)
  ret i32 0
}
