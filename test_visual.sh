#!/bin/bash
# Visual test runner for Breenix - runs tests with QEMU display enabled

echo "🚀 Breenix Visual Test Runner"
echo "============================="
echo ""

# Function to run a test with visual output
run_visual_test() {
    local test_name=$1
    local duration=${2:-5}
    local features=${3:-""}
    
    echo "🔍 Running: $test_name (${duration}s)"
    echo "----------------------------------------"
    
    if [ -n "$features" ]; then
        timeout ${duration}s cargo run --features "$features" --bin qemu-uefi -- -serial stdio
    else
        timeout ${duration}s cargo run --bin qemu-uefi -- -serial stdio
    fi
    
    echo ""
    echo "✅ Test completed"
    echo ""
}

# Parse command line arguments
HEADED_MODE=false
RUN_ALL=false
TEST_NAME=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --headed)
            HEADED_MODE=true
            shift
            ;;
        --all)
            RUN_ALL=true
            shift
            ;;
        --test)
            TEST_NAME="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--headed] [--all] [--test TEST_NAME]"
            echo "  --headed    Run with QEMU display window"
            echo "  --all       Run all tests sequentially"
            echo "  --test NAME Run specific test"
            exit 1
            ;;
    esac
done

# Set QEMU display options based on mode
if [ "$HEADED_MODE" = true ]; then
    export QEMU_DISPLAY=""
    echo "🖥️  Running in HEADED mode (QEMU window will appear)"
else
    export QEMU_DISPLAY="-display none"
    echo "🔇 Running in HEADLESS mode"
fi

echo ""

# If running all tests
if [ "$RUN_ALL" = true ]; then
    echo "Running all visual tests..."
    echo ""
    
    # Basic boot test
    run_visual_test "Basic Boot Test" 5
    
    # Memory test (watch allocation messages)
    run_visual_test "Memory Management Test" 5
    
    # Runtime tests
    run_visual_test "Runtime Testing Feature" 8 "testing"
    
    # Extended stability test
    run_visual_test "Extended Stability Test" 15
    
    echo "🎉 All visual tests completed!"
    
elif [ -n "$TEST_NAME" ]; then
    # Run specific test
    case $TEST_NAME in
        boot)
            run_visual_test "Boot Sequence Test" 5
            ;;
        memory)
            run_visual_test "Memory Management Test" 5
            ;;
        testing)
            run_visual_test "Runtime Testing Feature" 8 "testing"
            ;;
        stability)
            run_visual_test "Stability Test" 20
            ;;
        keyboard)
            echo "🎹 Keyboard Test - Press keys to see scancodes!"
            run_visual_test "Keyboard Input Test" 30
            ;;
        *)
            echo "Unknown test: $TEST_NAME"
            echo "Available tests: boot, memory, testing, stability, keyboard"
            exit 1
            ;;
    esac
else
    # Interactive mode
    echo "🎯 Interactive Test Mode"
    echo "======================="
    echo ""
    echo "Select a test to run:"
    echo "1) Boot Sequence Test (5s)"
    echo "2) Memory Management Test (5s)"
    echo "3) Runtime Testing Feature (8s)"
    echo "4) Stability Test (20s)"
    echo "5) Keyboard Input Test (30s)"
    echo "6) Run All Tests"
    echo ""
    read -p "Enter choice (1-6): " choice
    
    case $choice in
        1) run_visual_test "Boot Sequence Test" 5 ;;
        2) run_visual_test "Memory Management Test" 5 ;;
        3) run_visual_test "Runtime Testing Feature" 8 "testing" ;;
        4) run_visual_test "Stability Test" 20 ;;
        5) 
            echo "🎹 Press keys during the test to see scancodes!"
            run_visual_test "Keyboard Input Test" 30 
            ;;
        6) 
            RUN_ALL=true
            $0 --all $([ "$HEADED_MODE" = true ] && echo "--headed")
            ;;
        *) echo "Invalid choice"; exit 1 ;;
    esac
fi