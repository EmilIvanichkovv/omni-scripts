# Branch Search Feature Specification

## Overview

Add real-time branch name search functionality to filter the branch list as the user types. This enables quick navigation in repositories with many branches.

## User Experience

### Activation
- Press `/` to enter search mode (standard TUI search convention)
- A search input field appears at the top of the branch list
- The cursor is placed in the search field

### Search Behavior
- **Real-time filtering**: Branch list filters as user types
- **Case-insensitive**: Search matches regardless of case
- **Substring match**: Matches anywhere in the branch name
- **Combined with filters**: Search works alongside status filters (merged, gone, etc.)

### Deactivation
- Press `Escape` to exit search mode and clear the search query
- Press `Enter` to exit search mode but keep the filter active

### Visual Indicators
- Search input field visible when search is active
- Current search query displayed in the input
- Match count shown (e.g., "3 matches")

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `/` | Enter search mode |
| `Escape` | Exit search, clear query |
| `Enter` | Exit search, keep query |
| `Backspace` | Delete last character |
| Any printable | Add to search query |

## Technical Design

### State Changes (`app.rs`)

Add to `App` struct:
```rust
/// Whether search mode is active (input focused)
pub search_active: bool,
/// Current search query string
pub search_query: String,
```

### Filter Logic

Modify `filtered_branches()` to apply search filter:
```rust
pub fn filtered_branches(&self) -> Vec<&BranchInfo> {
    let status_filtered = // existing filter logic
    
    if self.search_query.is_empty() {
        status_filtered
    } else {
        let query = self.search_query.to_lowercase();
        status_filtered
            .into_iter()
            .filter(|b| b.name.to_lowercase().contains(&query))
            .collect()
    }
}
```

### UI Changes (`ui.rs`)

- Add search input rendering when `app.search_active` or `!app.search_query.is_empty()`
- Display search query and match count
- Visual highlight when search is active

### Key Handling (`main.rs`)

- `/` toggles search mode on
- When search active: capture printable chars, backspace, escape, enter
- Propagate changes to `app.search_query`

## Edge Cases

1. **Empty search**: Show all branches (within current filter)
2. **No matches**: Show empty list with "No matches" message
3. **Filter change during search**: Reapply both filters
4. **Selection during search**: Selection indices map to filtered list

## Migration Notes

The `/` key was previously used for filter bar toggle. This needs to be reassigned:
- Filter bar toggle moves to `F` key (mnemonic: **F**ilter)
- `/` becomes search (standard convention in vim, less, etc.)

## Testing

- [ ] Search filters branches correctly
- [ ] Case-insensitive matching works
- [ ] Combined with status filters
- [ ] Escape clears and exits
- [ ] Enter keeps query and exits
- [ ] Empty query shows all branches
- [ ] Selection works with search active
