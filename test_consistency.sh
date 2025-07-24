#!/bin/bash

# Test script to run Breenix 5 times and check for userspace execution success

echo "Running Breenix 5 times to test consistency..."
echo "================================================"

success_count=0
fail_count=0

for i in {1..5}; do
    echo ""
    echo "Run #$i starting at $(date)"
    echo "------------------------"
    
    # Kill any existing QEMU processes
    pkill -9 -f qemu-system-x86_64 2>/dev/null
    sleep 1
    
    # Run Breenix with display none and capture output
    timeout 30 ./scripts/run_breenix.sh uefi -display none > run_$i.log 2>&1
    
    # Check if userspace executed successfully
    if grep -q "USERSPACE OUTPUT: Hello from userspace" run_$i.log; then
        echo "✅ Run #$i: SUCCESS - Userspace executed"
        ((success_count++))
        
        # Also check for clean exit
        if grep -q "sys_exit called with code: 0" run_$i.log; then
            echo "   Clean exit detected"
        fi
    else
        echo "❌ Run #$i: FAILED - No userspace execution detected"
        ((fail_count++))
        
        # Show last few lines for debugging
        echo "   Last 10 lines of output:"
        tail -10 run_$i.log | sed 's/^/   /'
    fi
    
    # Kill QEMU after each run
    pkill -9 -f qemu-system-x86_64 2>/dev/null
    sleep 2
done

echo ""
echo "================================================"
echo "SUMMARY:"
echo "Success: $success_count/5"
echo "Failed:  $fail_count/5"
echo "================================================"

# Cleanup
rm -f run_*.log