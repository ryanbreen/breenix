# Breenix Skills Marketplace

This repository contains a local Claude Code skills marketplace specifically for Breenix OS kernel development. These skills encapsulate workflows, patterns, and best practices for developing and debugging a production-quality x86_64 operating system kernel in Rust.

## Installation

To use these skills with Claude Code:

```bash
# Register the marketplace
/plugin marketplace add /Users/wrb/fun/code/breenix

# Install the plugins
/plugin install breenix-ci@breenix-skills
/plugin install breenix-development@breenix-skills
/plugin install breenix-testing@breenix-skills
```

## Skill Categories

### 1. CI/CD Skills (breenix-ci)

**breenix-github-workflow-authoring**
- Creating and improving GitHub Actions workflows for kernel testing
- Understands Breenix-specific patterns: QEMU, Rust nightly, userspace builds
- Handles timeout strategies, caching, and environment setup
- Provides templates and best practices

**breenix-ci-failure-analysis**
- Systematic analysis of failed GitHub Actions runs
- Automated pattern detection for common failures (double faults, page faults, timeouts)
- Includes `analyze_ci_failure.py` script for log analysis
- Provides diagnosis and remediation steps

### 2. Development Skills (breenix-development)

**breenix-kernel-debug-loop**
- Fast iterative kernel debugging with time-bounded execution (default 15s)
- Real-time log monitoring for checkpoint signals
- Immediate termination when expected signal detected
- Includes `quick_debug.py` script for rapid feedback cycles

**breenix-log-analysis**
- Searching and analyzing timestamped kernel logs
- Finding checkpoint signals and tracing execution flow
- Understanding log patterns and extracting diagnostic information
- Works with `scripts/find-in-logs` tool

**breenix-systematic-debugging**
- Document-driven debugging following Problem→Root Cause→Solution→Evidence pattern
- Templates for documenting complex kernel issues
- Integration with other debugging tools
- Based on successful debugging docs (TIMER_INTERRUPT_INVESTIGATION.md, etc.)

**breenix-code-quality-check**
- Pre-commit quality verification (zero warnings policy)
- Clippy configuration and common warning fixes
- Log side-effect detection
- Enforces Breenix coding standards from CLAUDE.md

**breenix-memory-debugging**
- Debugging page faults, double faults, and allocator issues
- Page table analysis and virtual memory debugging
- Frame allocator and heap debugging techniques
- Common memory error patterns with diagnosis and fixes

**breenix-boot-analysis**
- Analyzing kernel boot sequence and initialization order
- Boot checkpoint verification and timing analysis
- Diagnosing boot hangs and failures
- Boot time optimization techniques

**breenix-legacy-migration**
- Systematic migration from src.legacy/ to new kernel
- Feature parity verification and legacy code removal
- FEATURE_COMPARISON.md maintenance
- Safe code retirement following Breenix principles

### 3. Testing Skills (breenix-testing)

**breenix-integration-test-authoring**
- Creating integration tests using shared QEMU pattern
- Checkpoint signal patterns and userspace test programs
- xtask command creation for complex tests
- CI workflow integration for automated testing

## Quick Reference

### Fast Debug Iteration
```bash
breenix-kernel-debug-loop/scripts/quick_debug.py \
  --signal "KERNEL_INITIALIZED" \
  --timeout 15
```

### Analyze CI Failure
```bash
breenix-ci-failure-analysis/scripts/analyze_ci_failure.py \
  --context target/xtask_ring3_smoke_output.txt
```

### Search Logs
```bash
echo '-A20 "Creating user process"' > /tmp/log-query.txt
./scripts/find-in-logs
```

### Pre-Commit Quality Check
```bash
cd kernel
cargo clippy --target x86_64-unknown-none
cargo build --target x86_64-unknown-none 2>&1 | grep warning
```

## Skill Invocation

Claude will automatically invoke these skills when appropriate based on your tasks:

- **Debugging kernel issues** → kernel-debug-loop, log-analysis
- **CI/CD failures** → ci-failure-analysis, github-workflow-authoring
- **Creating tests** → integration-test-authoring
- **Code review prep** → code-quality-check
- **Complex bug investigation** → systematic-debugging

You can also explicitly reference skills in your prompts:
- "Use the ci-failure-analysis skill to analyze this log"
- "Help me create a new integration test using the integration-test-authoring skill"

## Scripts Provided

### breenix-kernel-debug-loop/scripts/quick_debug.py
Fast kernel debugging with signal detection and timeout management.

### breenix-ci-failure-analysis/scripts/analyze_ci_failure.py
Automated CI log analysis with pattern detection and remediation suggestions.

## Documentation Structure

Each skill includes:
- **SKILL.md**: Complete skill documentation with patterns and examples
- **scripts/**: Executable tools for the skill
- **references/**: Additional reference material and templates

## Integration with Breenix Workflow

These skills integrate with Breenix development practices:

1. **Feature Development**: Use kernel-debug-loop for rapid iteration
2. **Code Quality**: Use code-quality-check before commits
3. **Testing**: Use integration-test-authoring for new features
4. **CI/CD**: Use github-workflow-authoring for new workflows
5. **Debugging**: Use systematic-debugging for complex issues
6. **Failure Analysis**: Use ci-failure-analysis for CI problems
7. **Log Analysis**: Use log-analysis throughout development

## Best Practices

1. **Run quick_debug.py during feature development** for fast feedback
2. **Analyze CI failures immediately** with the analysis script
3. **Document complex bugs** using systematic-debugging pattern
4. **Check code quality before every commit**
5. **Use checkpoint signals** in all tests for reliable detection
6. **Reference skill documentation** for workflow patterns

## Extending the Marketplace

To add new skills:

1. Create skill directory with `SKILL.md`
2. Add optional `scripts/` and `references/` directories
3. Update `.claude-plugin/marketplace.json`
4. Test the skill by invoking it
5. Document in this README

## Skill Design Principles

These skills follow the Anthropic skills design philosophy:

- **Progressive disclosure**: Metadata → SKILL.md → Resources
- **Self-contained**: Each skill can work independently
- **Scriptable**: Complex operations in Python/Bash scripts
- **Documented**: Clear when-to-use guidance
- **Integrated**: Skills work together for comprehensive workflows

## Version History

**v1.0.0** (2025-10-20)
- Initial marketplace creation
- 7 core skills across CI/CD, development, and testing
- Scripts for automated analysis and debugging
- Complete integration with Breenix workflows

## Support

For issues or questions about these skills:
- Check skill documentation in `[skill-name]/SKILL.md`
- Review script source in `[skill-name]/scripts/`
- Consult reference materials in `[skill-name]/references/`
- Reference CLAUDE.md for Breenix development standards

---

*These skills are specific to Breenix OS development and complement the general-purpose skills from anthropic-agent-skills.*
