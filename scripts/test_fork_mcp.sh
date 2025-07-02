#!/bin/bash
#
# Automated fork testing using MCP commands for Breenix kernel
# This script starts the kernel, sends fork test commands, and evaluates logs
#

set -e

ITERATIONS=${1:-5}
TEST_LOG="fork_test_results.txt"

echo "🧪 Breenix Fork Test Automator (MCP)"
echo "=" | tr -s '=' | head -c 40; echo
echo "Running $ITERATIONS test iterations"
echo

# Initialize log file
echo "Fork Test Results - $(date)" > "$TEST_LOG"
echo "==============================" >> "$TEST_LOG"

# Function to log both to console and file
log_both() {
    echo "$1" | tee -a "$TEST_LOG"
}

# Function to test fork functionality
run_fork_test() {
    local test_num=$1

    log_both ""
    log_both "=== Fork Test #$test_num ==="

    # Kill any existing kernel instances
    echo "🛑 Killing existing kernel instances..."
    claude mcp call breenix_kill || true
    sleep 1

    # Start the kernel
    echo "🚀 Starting Breenix kernel..."
    if ! claude mcp call breenix_start '{"display": false}'; then
        log_both "❌ Failed to start kernel"
        return 1
    fi

    # Wait for kernel to boot
    echo "⏳ Waiting for kernel boot..."
    sleep 3

    # Wait for prompt
    echo "⏳ Waiting for prompt..."
    if ! claude mcp call breenix_wait_prompt '{"timeout": 10}'; then
        log_both "❌ Kernel prompt not ready"
        return 1
    fi

    log_both "✅ Kernel started and ready"

    # Send fork test command via serial console
    echo "🧪 Sending fork test command..."
    if ! claude mcp call breenix_send '{"command": "forktest"}'; then
        log_both "❌ Failed to send fork test command"
        return 1
    fi

    log_both "📤 Sent 'forktest' command to trigger fork test"

    # Wait for test execution
    echo "⏳ Waiting for test execution..."
    sleep 2

    # Collect and analyze logs
    echo "📋 Collecting kernel logs..."
    if ! claude mcp call breenix_logs '{"lines": 100}' > "test_${test_num}_logs.txt"; then
        log_both "❌ Failed to collect logs"
        return 1
    fi

    # Analyze the logs
    analyze_fork_logs "test_${test_num}_logs.txt" "$test_num"

    # Stop the kernel
    echo "🛑 Stopping kernel..."
    claude mcp call breenix_stop || true
    sleep 1

    return 0
}

# Function to analyze fork test logs
analyze_fork_logs() {
    local log_file=$1
    local test_num=$2

    log_both "🔍 Analyzing logs for test #$test_num..."

    # Check for key patterns in the logs
    local test_started=false
    local fork_called=false
    local thread_id_tracked=false
    local process_created=false
    local errors_found=false

    if grep -q "Testing Fork System Call.*Debug Mode" "$log_file"; then
        test_started=true
        log_both "✅ Fork test started"
    fi

    if grep -q "Created and scheduled fork debug process.*PID" "$log_file"; then
        process_created=true
        log_both "✅ Fork test process created"
    fi

    if grep -q "sys_fork called" "$log_file"; then
        fork_called=true
        log_both "✅ Fork system call invoked"
    fi

    if grep -q "Scheduler reports current thread ID:" "$log_file"; then
        thread_id_tracked=true
        local thread_id=$(grep "Scheduler reports current thread ID:" "$log_file" | head -1 | sed 's/.*ID: \([0-9]*\).*/\1/')
        log_both "✅ Thread ID tracked: $thread_id"
    fi

    if grep -q "sys_fork: Not implemented yet" "$log_file"; then
        log_both "ℹ️  Fork not implemented (expected)"
    fi

    if grep -q "returning error" "$log_file"; then
        errors_found=true
        log_both "⚠️  Error in fork implementation"
    fi

    # Extract userspace output
    if grep -q "USERSPACE OUTPUT:" "$log_file"; then
        log_both "📝 Userspace output found:"
        grep "USERSPACE OUTPUT:" "$log_file" | sed 's/.*USERSPACE OUTPUT: /  /' | tee -a "$TEST_LOG"
    fi

    # Determine test result
    if $test_started && $fork_called && $thread_id_tracked; then
        log_both "✅ Test #$test_num: SUCCESS - Fork test executed with thread tracking"
        echo "SUCCESS" >> "test_${test_num}_result.txt"
    elif $test_started && $fork_called; then
        log_both "⚠️  Test #$test_num: PARTIAL - Fork called but thread tracking issue"
        echo "PARTIAL" >> "test_${test_num}_result.txt"
    else
        log_both "❌ Test #$test_num: FAILED - Fork test did not execute properly"
        echo "FAILED" >> "test_${test_num}_result.txt"
    fi

    # Save detailed analysis
    {
        echo "Test #$test_num Analysis:"
        echo "  Test Started: $test_started"
        echo "  Process Created: $process_created"
        echo "  Fork Called: $fork_called"
        echo "  Thread ID Tracked: $thread_id_tracked"
        echo "  Errors Found: $errors_found"
        echo ""
    } >> "$TEST_LOG"
}

# Function to print summary
print_summary() {
    local total_tests=$1

    log_both ""
    log_both "=== Fork Test Summary ($total_tests tests) ==="

    local success_count=0
    local partial_count=0
    local failed_count=0

    for i in $(seq 1 $total_tests); do
        if [[ -f "test_${i}_result.txt" ]]; then
            local result=$(cat "test_${i}_result.txt")
            case $result in
                "SUCCESS")
                    success_count=$((success_count + 1))
                    log_both "Test $i: ✅ SUCCESS"
                    ;;
                "PARTIAL")
                    partial_count=$((partial_count + 1))
                    log_both "Test $i: ⚠️  PARTIAL"
                    ;;
                "FAILED")
                    failed_count=$((failed_count + 1))
                    log_both "Test $i: ❌ FAILED"
                    ;;
            esac
        else
            failed_count=$((failed_count + 1))
            log_both "Test $i: ❌ NO RESULT"
        fi
    done

    log_both ""
    log_both "Statistics:"
    log_both "  Successful: $success_count/$total_tests"
    log_both "  Partial: $partial_count/$total_tests"
    log_both "  Failed: $failed_count/$total_tests"

    if [[ $total_tests -gt 0 ]]; then
        local success_rate=$((success_count * 100 / total_tests))
        log_both "  Success rate: $success_rate%"
    fi
}

# Cleanup function
cleanup() {
    echo ""
    echo "🧹 Cleaning up..."
    claude mcp call breenix_stop 2>/dev/null || true
    claude mcp call breenix_kill 2>/dev/null || true

    # Clean up temporary files
    for i in $(seq 1 $ITERATIONS); do
        rm -f "test_${i}_logs.txt" "test_${i}_result.txt"
    done
}

# Set up signal handling
trap cleanup EXIT INT TERM

# Main execution
main() {
    # Check if MCP commands are available
    if ! command -v claude >/dev/null 2>&1; then
        echo "❌ Claude CLI not found. Please install and configure it first."
        exit 1
    fi

    # Test MCP connection
    echo "🔗 Testing MCP connection..."
    if ! claude mcp call breenix_running >/dev/null 2>&1; then
        echo "❌ MCP Breenix server not available. Please check your MCP configuration."
        exit 1
    fi

    echo "✅ MCP connection verified"

    # Run test iterations
    local failed_tests=0

    for i in $(seq 1 $ITERATIONS); do
        if ! run_fork_test "$i"; then
            failed_tests=$((failed_tests + 1))
            log_both "❌ Test #$i failed"
        fi

        # Brief pause between tests (except for last test)
        if [[ $i -lt $ITERATIONS ]]; then
            sleep 1
        fi
    done

    # Print summary
    print_summary "$ITERATIONS"

    # Final status
    if [[ $failed_tests -eq 0 ]]; then
        log_both ""
        log_both "🎉 All tests completed successfully!"
        exit 0
    else
        log_both ""
        log_both "⚠️  $failed_tests tests failed or had issues"
        exit 1
    fi
}

# Run main function
main "$@"