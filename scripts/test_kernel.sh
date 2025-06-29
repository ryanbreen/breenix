#!/bin/bash
# Simple test runner for Breenix

echo "🧪 Breenix Test Suite"
echo "===================="
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Build kernel
echo -e "${YELLOW}Building kernel...${NC}"
cd kernel && cargo +nightly build --target ../x86_64-breenix.json -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --quiet 2>/dev/null
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✅ Kernel build successful${NC}"
else
    echo -e "${RED}❌ Kernel build failed${NC}"
    exit 1
fi
cd ..

# Build QEMU runners  
echo -e "${YELLOW}Building QEMU runners...${NC}"
cargo build --bin qemu-uefi --quiet 2>/dev/null
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✅ QEMU runners built${NC}"
else
    echo -e "${RED}❌ QEMU runner build failed${NC}"
    exit 1
fi

echo ""
echo -e "${YELLOW}Running kernel test...${NC}"
echo "This will run for ~10 seconds to collect output."
echo ""

# Use expect if available, otherwise use a simple background approach
if command -v expect >/dev/null 2>&1; then
    # Use expect to capture output
    expect -c '
        set timeout 10
        spawn cargo run --bin qemu-uefi -- -display none -serial stdio
        expect {
            "Interrupts enabled!" { 
                send_user "\nKernel initialized successfully!\n"
                exit 0
            }
            timeout { 
                send_user "\nTimeout waiting for kernel initialization\n"
                exit 1
            }
        }
    ' 2>/dev/null
else
    # Simple approach: just run it and let user see output
    echo "Running kernel (will auto-terminate in 10 seconds)..."
    echo "Look for these messages to confirm everything is working:"
    echo "  ✓ Kernel entry point reached"
    echo "  ✓ Serial port initialized"
    echo "  ✓ Memory management initialized"
    echo "  ✓ Interrupts enabled!"
    echo ""
    echo "Output:"
    echo "-------"
    
    # Run with a simple bash timeout
    (
        cargo run --bin qemu-uefi 2>/dev/null -- -display none -serial stdio &
        PID=$!
        sleep 10
        kill $PID 2>/dev/null
    )
    
    echo ""
    echo "-------"
    echo ""
    echo -e "${GREEN}Test complete!${NC}"
    echo ""
    echo "If you saw all the initialization messages above, the kernel is working correctly."
    echo "If not, try running manually:"
    echo "  cargo run --bin qemu-uefi -- -display none -serial stdio"
fi