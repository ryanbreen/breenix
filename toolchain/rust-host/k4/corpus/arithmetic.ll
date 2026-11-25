; LLVM Language Reference: integer binary operations.
source_filename = "arithmetic.ll"
define i64 @arithmetic(i64 %a, i64 %b) {
entry:
  %sum = add i64 %a, %b
  %product = mul i64 %sum, 7
  %result = xor i64 %product, 42
  ret i64 %result
}
