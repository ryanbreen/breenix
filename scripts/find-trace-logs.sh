#!/bin/bash
# Find all trace log statements and analyze them for potential issues

echo "=== Finding all log::trace! statements in kernel ==="
echo ""

# Find all files with trace logs
FILES=$(grep -r "log::trace!" kernel/src/ -l)

for file in $FILES; do
    echo "File: $file"
    echo "----------------------------------------"
    
    # Show trace logs with context
    grep -n "log::trace!" "$file" -B2 -A2 | head -20
    
    echo ""
done

echo ""
echo "=== Checking for potential side effects ==="
echo ""

# Look for trace logs that might call functions
echo "Trace logs with function calls (excluding simple getters):"
grep -r "log::trace!.*(" kernel/src/ | grep -v '".*"' | grep -vE '\.(as_|to_|into_|len|is_|get|start_address|as_u64)' || echo "None found"

echo ""
echo "=== Summary ==="
echo "Total files with trace logs: $(echo "$FILES" | wc -l)"
echo "Total trace log statements: $(grep -r "log::trace!" kernel/src/ | wc -l)"