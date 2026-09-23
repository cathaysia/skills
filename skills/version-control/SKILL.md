---
name: version-control
description: Enforce strict linear Git history across all version control operations. Use whenever creating commits, branching, syncing, rebasing, merging, or pushing changes.
---

# Version Control (Strict Linear History)

Always maintain a strictly linear Git history ($A \to B \to C$). Never introduce
merge commits into target or trunk branches.

## Rules

- **No merge commits**: Never create 3-way merge commits (`Merge branch...`).
  Foxtrot and diamond merges are forbidden.
- **Sync via rebase**: When updating feature branches with upstream/trunk
  changes, always rebase (`git pull --rebase` or
  `git fetch && git rebase origin/<target>`). Never `git merge` the target
  branch into your feature branch.
- **Integrate via fast-forward or squash**:
  - Keep separate commits: `git merge --ff-only <branch>` (rebase onto target
    first if needed).
  - Single commit PR/merge: `git merge --squash <branch> && git commit`.
- **Clean atomic commits**: Squash WIP, typo, and fixup commits before merging
  (`git rebase -i`). Every commit on trunk must be functional and clean.
- **Amend unpushed fixes**: If changes are meant to fix the previous commit and
  that commit has not been pushed yet, amend it directly (`git commit --amend`
  or `git commit --amend --no-edit`) instead of creating a new commit.
- **Safe push**: Always use `git push --force-with-lease` when updating rebased
  remote feature branches. Never force-push to shared/protected branches
  (`main`, `master`).
- **Linear undo**: Revert mistakes on shared branches with `git revert`, never
  by rewriting shared history.
- **Conflict resolution**: Resolve conflict markers $\to$ `git add <files>`
  $\to$ `git rebase --continue`. Do not run `git commit` mid-rebase.
