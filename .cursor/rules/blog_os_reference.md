---
include: always
glob: "**/*"
description: "Fundamental rule governing the use of blog_os repository as reference material, ensuring it remains read-only and properly referenced across all development sessions."
---

# Blog OS Reference Rule

## Purpose
This rule establishes the fundamental guidelines for using the blog_os repository as a reference in our development process.

## Rule Details
1. The blog_os directory and its contents are to be used exclusively as reference material
2. No direct edits should be made to the blog_os directory or its contents
3. Primary reference material is located in `blog/content/edition-3`
4. The blog_os repository serves as a learning and reference resource, not as a target for modifications

## Critical Note: Project Name
- When following blog_os instructions or commands, ALWAYS substitute "breenix" for "kernel"
- This applies to all commands, file paths, and code references
- Example: `cargo build --target kernel.json` becomes `cargo build --target breenix.json`
- Example: `kernel_main` becomes `breenix_main`

## Rationale
- Maintains the integrity of the original blog_os repository
- Ensures consistent reference material across development sessions
- Prevents accidental modifications to reference code
- Focuses development efforts on our own implementation

## Implementation
- All code references should be read-only
- When implementing features, use blog_os as a guide but implement independently
- Document any specific blog_os references used in implementation decisions