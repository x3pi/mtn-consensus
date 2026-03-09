import re

with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
    content = f.read()

# Replace missing Core::new args (adaptive delay and sys_tx)
# find all Core::new(...) instances and add `None, None` if they don't have 12 args.
# In core.rs tests, these are mostly `Core::new(..., round_tracker);` or `Core::new(..., true, round_tracker)`

content = re.sub(
    r'(Core::new\([^;]+\s*,\s*round_tracker(\.clone\(\))?\s*)\)',
    r'\1, None, None)',
    content,
    flags=re.M | re.S
)

# Replace missing CommitObserver::new args (epoch base index) -> `0`
# usually `leader_schedule.clone(), )`
content = re.sub(
    r'(CommitObserver::new\([^;]+leader_schedule(?:\.clone\(\))?\s*,\s*)\)',
    r'\1 0)',
    content,
    flags=re.M | re.S
)

with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
    f.write(content)

with open('metanode/meta-consensus/core/src/core_thread.rs', 'r') as f:
    content = f.read()

content = re.sub(
    r'(Core::new\([^;]+\s*,\s*round_tracker(\.clone\(\))?\s*)\)',
    r'\1, None, None)',
    content,
    flags=re.M | re.S
)

content = re.sub(
    r'(CommitObserver::new\([^;]+leader_schedule(?:\.clone\(\))?\s*,\s*)\)',
    r'\1 0)',
    content,
    flags=re.M | re.S
)

with open('metanode/meta-consensus/core/src/core_thread.rs', 'w') as f:
    f.write(content)

