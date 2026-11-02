// Skip the zero quotient prefix of the shared 96-bit restoring division.
// Nonzero upper words, a zero divisor, or runtime poison retain the original
// counter. The same operation graph is used by nv50_ir and the host check.
#pragma once

template<class Ops>
typename Ops::V kepler_line_division_start(Ops &op, typename Ops::V original,
    typename Ops::V low, typename Ops::V middle, typename Ops::V high,
    typename Ops::V divisor_low, typename Ops::V divisor_middle,
    typename Ops::V divisor_high, typename Ops::V poison)
{
    auto upper = op.or_(middle, op.or_(high, poison));
    auto divisor = op.or_(divisor_low, op.or_(divisor_middle, divisor_high));
    auto safe = op.and_(op.eqzero(upper), op.nonzero(divisor));
    auto bits = op.add(op.highest(op.or_(low, op.imm(1))), op.imm(1));
    return op.select(safe, bits, original);
}
