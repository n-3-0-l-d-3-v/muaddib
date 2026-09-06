# Constraints — THE KERNEL

## Primary constraint

There is no hierarchical path resolution anywhere in the system, no trusted wall clock, and all resource access happens through explicit capabilities.

## What it forces

Capability passing, IPC, logical/vector clocks for ordering, and object-reference-based resource naming instead of paths.

## Research question

Is a filesystem fundamentally a hierarchy, or is naming merely one possible interface to persistent objects? What can an OS do when physical time is unavailable as a coordination primitive?

## What is explicitly out of scope

See the root [SCOPE.md](../../SCOPE.md) for the CORE / EXTENSION / EXPERIMENT
classification that applies to this repo.
