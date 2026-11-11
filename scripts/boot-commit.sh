#!/bin/bash
# Print the identity of the checkout or deployed archive in the current directory.
boot_commit() {
    local sha dirty
    if [ -e .git ]; then
        sha=$(git rev-parse HEAD) || return 2
        dirty=0
        [ -z "$(git status --porcelain)" ] || dirty=1
        printf '%s dirty=%s\n' "$sha" "$dirty"
    elif [ -f .deployed-commit ]; then
        sha=$(cat .deployed-commit) || return 2
        if [[ "$sha" =~ ^[0-9a-fA-F]{40}$ ]]; then
            printf '%s deployed=1\n' "$sha"
            return 0
        fi
        echo 'no commit identity' >&2
        return 2
    else
        echo 'no commit identity' >&2
        return 2
    fi
}
