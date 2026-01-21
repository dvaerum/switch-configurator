# Error Handling Implementation Plan

**Status**: Ready to implement
**Estimated Time**: 2-3 hours
**Complexity**: Medium

## What We're Solving

Currently, YAML parsing errors are generic and unhelpful:
```
Failed to parse config file: "/etc/switch-configurator/switch-config.yaml"
invalid type: string "33,34,35,36", expected a sequence
```

We want helpful, actionable errors:
```
ERROR Failed to parse config file: /etc/switch-configurator/switch-config.yaml
  Location: Line 3, field 'port_mirrors[0].source_ports'
  Problem: Type mismatch - string provided, array expected
  Expected: Array of port IDs (e.g., ["33", "34", "35", "36"])
  Found: String value "33,34,35,36"
  Fix: Change to: source_ports: ["33", "34", "35", "36"]
```

## Nothing is Stopping Us!

**Dependencies**:
- ✅ `serde_yaml` already in Cargo.toml
- ✅ `anyhow` already in Cargo.toml
- ⏭️ Need to add: `serde_path_to_error` (simple cargo add)

**Risks**: Low
- Only modifying error handling, not core logic
- Can iterate gradually (start simple, enhance later)
- Existing tests will catch any breaks

**Prerequisites**: All met
- ✅ All tests passing
- ✅ Clean working tree
- ✅ Good understanding of config loading flow

## Implementation Plan

### Phase 1: Add Dependency (5 minutes)

**Action**: Add `serde_path_to_error` to Cargo.toml
```bash
cargo add serde_path_to_error
```

**Verification**: Cargo build succeeds

---

### Phase 2: Wrap Deserialization with Path Tracking (30 minutes)

**File**: `src/config.rs`
**Target**: Lines 101-102 where `serde_yaml::from_str` is called

**Current Code**:
```rust
let mut config: AppConfig = serde_yaml::from_str(&content)
    .with_context(|| format!("Failed to parse config file: {:?}", path))?;
```

**New Code**:
```rust
let mut config: AppConfig = {
    let deserializer = serde_yaml::Deserializer::from_str(&content);
    serde_path_to_error::deserialize(deserializer)
        .map_err(|err| {
            // Enhanced error with field path
            let field_path = err.path().to_string();
            anyhow!(
                "Failed to parse config file: {:?}\n\
                 Field path: {}\n\
                 Error: {}",
                path,
                field_path,
                err.into_inner()
            )
        })?
};
```

**Benefits**:
- Shows exact field path (e.g., `switches[0].credentials.username`)
- Maintains existing error context
- Minimal code change

**Testing**:
- Run existing tests (should all pass)
- Manually test with broken config (see field paths)

---

### Phase 3: Add Custom Error Messages for Common Cases (45 minutes)

**File**: Create new `src/config/errors.rs`

**Content**:
```rust
use std::fmt;

/// Enhanced config error with helpful messages
pub struct ConfigError {
    pub file_path: String,
    pub field_path: String,
    pub error_type: ConfigErrorType,
    pub raw_error: String,
}

pub enum ConfigErrorType {
    MissingField(String),      // field name
    TypeMismatch { expected: String, found: String },
    InvalidValue { value: String, reason: String },
    UnitValue(String),         // empty/null field
    InvalidEnum { value: String, valid_options: Vec<String> },
}

impl ConfigError {
    pub fn with_suggestions(self) -> String {
        match &self.error_type {
            ConfigErrorType::MissingField(field) => {
                format!(
                    "ERROR: Missing required field '{}'\n\
                     File: {}\n\
                     Field path: {}\n\
                     \n\
                     {}",
                    field,
                    self.file_path,
                    self.field_path,
                    self.suggestion_for_field(field)
                )
            }
            ConfigErrorType::TypeMismatch { expected, found } => {
                format!(
                    "ERROR: Type mismatch\n\
                     File: {}\n\
                     Field: {}\n\
                     Expected: {}\n\
                     Found: {}\n\
                     \n\
                     {}",
                    self.file_path,
                    self.field_path,
                    expected,
                    found,
                    self.fix_suggestion_for_type(expected, found)
                )
            }
            ConfigErrorType::UnitValue(field) => {
                format!(
                    "ERROR: Empty value provided for '{}'\n\
                     File: {}\n\
                     Field path: {}\n\
                     \n\
                     The field '{}' cannot be empty or null.\n\
                     {}",
                    field,
                    self.file_path,
                    self.field_path,
                    field,
                    self.suggestion_for_field(field)
                )
            }
            // ... more cases
            _ => self.raw_error,
        }
    }

    fn suggestion_for_field(&self, field: &str) -> String {
        match field {
            "credentials" => {
                "Fix: Add credentials section:\n\
                 credentials:\n\
                   username: admin\n\
                   password: yourpassword\n\
                 \n\
                 Or use SSH key:\n\
                 credentials:\n\
                   username: admin\n\
                   ssh_key_path: /path/to/key".to_string()
            }
            "management_ip" => {
                "Fix: Add management_ip:\n\
                 management_ip: \"192.168.1.10\"".to_string()
            }
            "source_ports" => {
                "Fix: Use array format:\n\
                 source_ports: [\"33\", \"34\", \"35\", \"36\"]".to_string()
            }
            _ => String::new(),
        }
    }

    fn fix_suggestion_for_type(&self, expected: &str, found: &str) -> String {
        if expected.contains("sequence") && found.contains("string") {
            "Fix: Change string to array format.\n\
             Example: Instead of source_ports: \"1,2,3\"\n\
             Use: source_ports: [\"1\", \"2\", \"3\"]".to_string()
        } else {
            String::new()
        }
    }
}
```

**Integration**:
- Update `src/config.rs` to use `ConfigError::with_suggestions()`
- Pattern match on error messages to detect error types
- Fall back to original error if no match

**Testing**:
- Create broken test configs for each error type
- Verify helpful messages appear
- Ensure existing tests still pass

---

### Phase 4: Add Line Number Support (30 minutes)

**Challenge**: `serde_yaml` doesn't provide line numbers by default

**Options**:
1. **Quick Win**: Use field path only (already have this from Phase 2)
2. **Better**: Parse errors manually from serde_yaml error messages (some contain line numbers)
3. **Best** (future): Switch to `serde_yaml_ng` which has better error reporting

**Recommendation**: Start with Option 1 (field path), add Option 2 if time permits

**Implementation** (Option 2):
```rust
fn extract_line_number(yaml_error: &str) -> Option<usize> {
    // serde_yaml errors sometimes contain "at line X column Y"
    let re = regex::Regex::new(r"at line (\d+)").ok()?;
    let captures = re.captures(yaml_error)?;
    captures.get(1)?.as_str().parse().ok()
}
```

---

### Phase 5: Testing & Validation (30 minutes)

**Test Cases to Create**:
1. Missing `management_ip`
2. Missing `credentials`
3. Empty/null `credentials:`
4. Type mismatch: `source_ports: "1,2,3"` (string instead of array)
5. Invalid enum value: `mode: wrong_mode`
6. Missing `id` field
7. Invalid model name

**Test Files**: Create in `tests/fixtures/invalid-configs/`
- `missing-management-ip.yaml`
- `missing-credentials.yaml`
- `empty-credentials.yaml`
- `source-ports-type-mismatch.yaml`
- `invalid-port-mode.yaml`

**Test Approach**:
```rust
#[test]
fn test_error_message_missing_management_ip() {
    let path = fixtures_path("invalid-configs/missing-management-ip.yaml");
    let result = AppConfig::load(&path);

    assert!(result.is_err());
    let error = result.unwrap_err().to_string();

    // Should mention the field name
    assert!(error.contains("management_ip"));
    // Should provide helpful context
    assert!(error.contains("Fix:") || error.contains("Example:"));
}
```

---

## Time Breakdown

| Phase | Task | Time | Running Total |
|-------|------|------|---------------|
| 1 | Add dependency | 5 min | 5 min |
| 2 | Wrap deserialization | 30 min | 35 min |
| 3 | Custom error messages | 45 min | 80 min |
| 4 | Line number support (optional) | 30 min | 110 min |
| 5 | Testing & validation | 30 min | 140 min |

**Total Estimated Time**: 2.5 hours (140 minutes)
**With buffer**: 3 hours

---

## Success Criteria

✅ **Must Have**:
1. Field path shown in all parse errors
2. Helpful suggestions for common errors:
   - Missing `credentials`
   - Missing `management_ip`
   - Type mismatches (especially `source_ports`)
3. All existing tests pass
4. At least 5 new error message tests

✅ **Nice to Have**:
1. Line numbers in errors
2. Examples in error messages
3. Colorized output (if terminal supports it)

❌ **Not in Scope**:
1. Schema-level validation (already done with `validator` crate)
2. Post-merge error improvements (separate task)
3. Custom deserializers for complex types

---

## Risks & Mitigation

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Breaking existing error handling | Low | Medium | Run full test suite after each phase |
| `serde_path_to_error` doesn't work with `serde_yaml` | Low | High | Test immediately in Phase 1 |
| Error messages too verbose | Medium | Low | Make detailed messages opt-in with `--verbose` flag |
| Can't extract line numbers | High | Low | Accept field path as minimum viable solution |

---

## Next Steps

**Ready to start?**

1. ✅ Plan complete
2. ⏭️ Run `cargo add serde_path_to_error`
3. ⏭️ Update `src/config.rs` deserialization
4. ⏭️ Create `src/config/errors.rs` module
5. ⏭️ Add test fixtures
6. ⏭️ Run tests
7. ⏭️ Commit when done

**Let's begin!** 🚀
