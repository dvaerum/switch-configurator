# Multi-Config Merge Design Q&A Summary

**Date:** 2025-01-21
**Status:** All questions answered, ready for implementation

This document summarizes the design clarification Q&A session that refined the multi-config merge feature design.

## Questions and Answers

### Q1: Settings Scope - Global or Per-Switch?
**Answer:** Per-switch settings

**Decision:** Move `settings` from `AppConfig` to `SwitchConfig`. Each switch has its own settings (ssh_timeout, max_retries, dry_run, enforce_port_config).

**Rationale:** More flexible - different switches may need different timeouts, retry counts, etc.

---

### Q2: Can Folder Configs Introduce New Switches?
**Answer:** Yes, allowed

**Decision:** Any config file can define new switches that don't exist in other files.

**Requirements:** New switches must have all required identity fields (id, hostname, management_ip, model, credentials).

---

### Q3: Can Main Config Have No Switches?
**Answer:** Yes, valid

**Decision:** Main config can be empty or have no switches defined.

**Use case:** Fully modular configurations where all switches are defined in folder configs, main.yaml is just a placeholder.

---

### Q4: When to Detect Conflicts?
**Answer:** Pre-merge validation pass

**Decision:** Before merging, scan all configs and collect ALL conflicts. Report all conflicts together, then abort before any merging.

**Rationale:** Users can fix multiple issues at once rather than iterative "whack-a-mole" debugging.

---

### Q5: How to Track Port Range Origins?
**Answer:** Add metadata during expansion

**Decision:** Track which ports came from ranges with metadata:
```rust
struct PortWithMetadata {
    port: Port,
    expanded_from_range: Option<String>,  // e.g., Some("1-5")
    source_file: PathBuf,
    priority: u16,
}
```

**Benefit:** Can provide detailed warnings when explicit port overrides port from range.

---

### Q6: SNMP Missing Sub-Components Handling?
**Answer:** Three distinct behaviors

**Decision:**
1. `snmp` field not present → use entire snmp from lower priority
2. `snmp: {}` (empty) → clear all SNMP config
3. `snmp` with some fields → merge sub-components independently

**Example:**
```yaml
# override.yaml (priority 30)
snmp:
  enabled_traps: ["mac-notify"]
  # communities and trap_receivers not specified

# Result: enabled_traps from override, communities and trap_receivers from lower priority
```

---

### Q7: Credentials Validation?
**Answer:** Require complete credentials

**Decision:** If a config specifies `credentials`, it must include all required fields:
- `username` (required)
- `password` OR `ssh_key_path` (required)
- `connection_type` (required)

**Rationale:** Prevent accidental authentication breakage from incomplete overrides.

---

### Q8: "Replace Entire Port" Behavior Confirmation?
**Answer:** Yes, confirmed

**Decision:** When replacing a port, all unspecified fields reset to defaults/None.

**Implication:** Users must specify ALL fields they want to keep in overrides.

**Example:**
```yaml
# main.yaml
ports:
  - port_id: "1"
    enabled: true
    vlan: 10
    poe_enabled: true
    description: "Server"

# override.yaml (higher priority)
ports:
  - port_id: "1"
    vlan: 20
    # enabled, poe_enabled, description: reset to defaults/None
```

---

### Q9: Port Range Expansion Timing?
**Answer:** During initial file load

**Decision:** Expand port ranges when loading each config file (current behavior at `src/config.rs:115`).

**Rationale:** Allows tracking range origins, enables better warnings during merge.

---

### Q10: Where Does merge_priority Live?
**Answer:** Separate wrapper struct

**Decision:** Don't add `merge_priority` to `AppConfig`. Use separate `ConfigWithMetadata` wrapper:
```rust
pub struct ConfigWithMetadata {
    pub merge_priority: u16,
    pub config: AppConfig,
    pub source_file: PathBuf,
    pub source_type: ConfigSourceType,
}
```

**Rationale:** Clean separation of concerns - AppConfig is the actual config, metadata is for merging.

---

### Q11: CLI Backward Compatibility?
**Answer:** Hard break

**Decision:** Remove `--config` entirely, only support `--config-file`. No deprecation period.

**Rationale:** User accepts breaking changes, keeps codebase clean.

---

### Q12: Conflict Error Reporting?
**Answer:** Show all conflicts at once

**Decision:** Collect all conflicts during pre-merge validation, report together:
```
Error: 3 merge conflicts detected:

1. Switch sw-01, VLAN 10, field 'name'
   Priority: 100
   - network.yaml: "management"
   - security.yaml: "mgmt"

2. Switch sw-01, Port "5", field 'enabled'
   ...

3. Switch sw-02, VLAN 20, field 'name'
   ...
```

**Rationale:** More efficient for users to fix all issues at once.

---

## Key Architectural Decisions Summary

1. **Settings:** Per-switch (breaking change)
2. **Switch Discovery:** Any file can introduce new switches
3. **Empty Configs:** Valid to have empty main config
4. **Conflict Detection:** Pre-merge validation, report all
5. **Port Tracking:** Metadata tracks range origins
6. **SNMP Merging:** Three-case behavior (absent/empty/partial)
7. **Credentials:** Must be complete if specified
8. **Object Replacement:** Full replacement, no field merging
9. **Range Expansion:** At load time
10. **Priority Storage:** Wrapper struct, not in AppConfig
11. **CLI Changes:** Hard break, no backward compatibility
12. **Error Reporting:** Show all conflicts together

---

## Implementation Impact

These decisions affect:
- **Phase 1:** Settings move, metadata wrapper design
- **Phase 2:** No backward compatibility code needed
- **Phase 3:** Pre-merge validation pass, metadata tracking, completeness validation
- **Phase 4:** Warning format for port range overlaps
- **Phase 5:** Migration guide complexity (3 breaking changes)

---

## Next Steps

1. ✅ Design document updated with all clarifications
2. ⏭️ Ready to begin Phase 1 implementation
3. 📋 34 tasks in todo list, organized by phase

---

## References

- Main design document: `docs/development/multi-config-merge-design.md`
- Task list: 34 tasks across 5 phases
- Breaking changes: 3 (id field, settings move, CLI rename)
