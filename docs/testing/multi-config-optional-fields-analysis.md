# Multi-Config Optional Fields Analysis

**Date**: November 24, 2025
**Status**: Tests Added, Schema Limitations Documented

## Summary

Added comprehensive tests for multi-config optional fields behavior. Discovered that `credentials` is currently **required at parse time** (not optional), which differs from TODO.md expectations.

## Current Behavior

### Credentials Field
- **Schema**: `pub credentials: Credentials` (NOT `Option<Credentials>`)
- **Parse-time**: Required in every YAML file that defines a switch
- **Merge-time**: N/A (validated before merge)
- **Location**: `src/models.rs:805`

### VLANs Field
- **Schema**: `pub vlans: Vec<Vlan>` (required, but can be empty `[]`)
- **Parse-time**: Required field, empty list allowed
- **Merge-time**: Can be empty after merge (no post-merge validation)
- **Location**: `src/models.rs:809`

### Ports Field
- **Schema**: `pub ports: Vec<Port>` (required, but can be empty `[]`)
- **Parse-time**: Required field, empty list allowed
- **Merge-time**: Can be empty after merge (no post-merge validation)
- **Location**: `src/models.rs:813`

### Optional Fields (with `#[serde(default)]`)
- `port_mirrors: Vec<PortMirror>` - Can be omitted
- `snmp: Option<SnmpConfig>` - Can be omitted
- `validation: Option<ValidationConfig>` - Can be omitted
- `settings: Option<SwitchSettings>` - Can be omitted

## Tests Added

### 1. ✅ `test_missing_credentials_in_all_configs_should_fail`
**Status**: `#[ignore]` - For future implementation
**Purpose**: Verify post-merge validation catches missing credentials
**Current Behavior**: Would fail at parse time (credentials required in schema)
**Future Behavior**: After schema change to `Option<Credentials>`, should fail at post-merge validation

### 2. ✅ `test_missing_vlans_in_all_configs_should_fail`
**Status**: `#[ignore]` - For future implementation
**Purpose**: Verify post-merge validation catches empty VLANs list
**Current Behavior**: Empty VLANs list is allowed
**Future Behavior**: Should fail with helpful error after merge

### 3. ✅ `test_credentials_provided_in_main_omitted_in_folder_succeeds`
**Status**: PASSING
**Purpose**: Verify credentials from main config work when folder omits them
**Behavior**: Credentials merge works correctly (highest priority wins)

### 4. ✅ `test_vlans_provided_in_folder_omitted_in_main_succeeds`
**Status**: PASSING
**Purpose**: Verify VLANs from folder work when main has empty list
**Behavior**: VLANs merge correctly from folder configs

### 5. ✅ `test_credentials_optional_during_parsing_required_after_merge`
**Status**: PASSING
**Purpose**: Document current behavior for future reference
**Behavior**: Currently fails at parse time if credentials missing
**Note**: Test documents what SHOULD happen after schema change

### 6. ✅ `test_vlans_optional_during_parsing_required_after_merge`
**Status**: PASSING
**Purpose**: Document current behavior for empty VLANs
**Behavior**: Empty VLANs currently allowed, documents expected future behavior

## Test Fixtures Created

### Missing Credentials Fixtures
```
tests/fixtures/multi-config/missing-credentials-all/
├── main.yaml          # No credentials field
└── common/
    └── ports.yaml     # No credentials field
```

### Missing VLANs Fixtures
```
tests/fixtures/multi-config/missing-vlans-all/
├── main.yaml          # Empty vlans: []
└── common/
    └── empty.yaml     # Empty vlans: [], port references VLAN 1
```

## Findings

### 1. Credentials Already Validated at Parse Time
**TODO.md** suggested credentials should be "optional in individual files, required after merge."

**Reality**: Credentials is NOT optional - it's a required field in the `SwitchConfig` struct. Any YAML file defining a switch MUST include credentials.

**Implication**: To achieve the TODO.md goal, need schema change:
```rust
// Current (parse-time required)
pub credentials: Credentials,

// Proposed (parse-time optional, post-merge required)
#[serde(default)]
pub credentials: Option<Credentials>,
```

Then add post-merge validation to ensure credentials exist after all merges complete.

### 2. VLANs Can Be Empty
Empty `vlans: []` is currently allowed. No switch requires at least one VLAN.

**Consideration**: Should we require at least one VLAN? Most switches need VLAN 1 (default) at minimum.

### 3. Multi-Config Merge Works for Truly Optional Fields
Fields with `#[serde(default)]` work as expected:
- Can be omitted in any file
- Highest priority file providing the field wins
- Examples: `port_mirrors`, `snmp`, `validation`, `settings`

## Recommendations

### Short Term (Current Session)
- ✅ Tests added and passing (24/24 pass, 2 ignored for future)
- ✅ Documented current behavior vs TODO.md expectations
- ✅ Created test fixtures for future validation work

### Medium Term (Separate Task)
1. **Schema Change**: Make `credentials` optional at parse time
   - Change to `Option<Credentials>` with `#[serde(default)]`
   - Update tests/fixtures that currently fail without credentials
   - Ensure existing tests still pass

2. **Post-Merge Validation**: Add validation after merge completes
   - Check credentials exist: `if switch.credentials.is_none() { error! }`
   - Check VLANs non-empty: `if switch.vlans.is_empty() { error! }`
   - Provide helpful error messages with switch ID and hostname
   - Enable the 2 ignored tests

3. **Better Error Messages**: Enhance validation errors (TODO task #4)
   - Show which config files were merged
   - Indicate which file should provide missing field
   - Suggest checking switch ID matches across files

### Long Term
- Consider validation for:
  - At least one port configured (or explicitly allow zero ports)
  - Credentials have either password OR ssh_key_path
  - All ports reference defined VLANs (already done)
  - Switch model matches actual hardware capabilities

## Related TODO.md Tasks

- ✅ Task #3: Create unit tests for multi-config optional fields - **COMPLETED**
- ⏭️ Task #4: Better error handling - **LARGE TASK, DEFERRED**
- ⏭️ Task #5: Post-merge validation - **REQUIRES SCHEMA CHANGE FIRST**

## Test Results

```
running 26 tests
test result: ok. 24 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**Total project tests**: 184 unit tests + 24 multi-config tests = 208 tests (100% passing, 2 ignored for future work)

## Files Modified

- `tests/multi_config_tests.rs`: Added 6 new tests (lines 435-590)
- `tests/fixtures/multi-config/missing-credentials-all/`: New fixture directory
- `tests/fixtures/multi-config/missing-vlans-all/`: New fixture directory
- `docs/testing/multi-config-optional-fields-analysis.md`: This document

## Conclusion

Tests successfully added to document current behavior and prepare for future post-merge validation work. Discovered that credentials is already required at parse time, which differs from TODO.md expectations. To achieve the desired "optional in files, required after merge" behavior, a schema change is required first.

The test infrastructure is now in place to validate post-merge behavior once the schema changes are implemented.
