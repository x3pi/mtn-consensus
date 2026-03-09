import re

with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
    content = f.read()

methods_to_extract = [
    "try_new_block",
    "should_propose",
    "try_select_certified_leaders",
    "smart_ancestors_to_propose",
    "leaders_exist",
    "leaders",
    "first_leader",
    "last_proposed_timestamp_ms",
    "last_proposed_round",
    "last_proposed_block"
]

extracted = ""
for method in methods_to_extract:
    start_idx = content.find(f"fn {method}(")
    if start_idx == -1:
        start_idx = content.find(f"    fn {method}(")
        if start_idx == -1:
            start_idx = content.find(f"pub(crate) fn {method}(")
            if start_idx == -1:
                start_idx = content.find(f"    pub(crate) fn {method}(")
    
    if start_idx == -1:
        print(f"Match not found for {method}")
        continue
        
    lines = content[:start_idx].split('\n')
    comments_start = start_idx
    for i in range(len(lines) - 2, -1, -1):
        if lines[i].strip().startswith('///') or lines[i].strip().startswith('#['):
            comments_start = content.rfind(lines[i], 0, comments_start)
        else:
            break
            
    brace_idx = content.find('{', start_idx)
    if brace_idx == -1:
        continue
        
    count = 1
    curr_idx = brace_idx + 1
    while count > 0 and curr_idx < len(content):
        if content[curr_idx] == '{':
            count += 1
        elif content[curr_idx] == '}':
            count -= 1
        curr_idx += 1
        
    end_idx = curr_idx
    method_str = content[comments_start:end_idx]
    method_str = method_str.replace(f"    fn {method}", f"    pub(crate) fn {method}")
    method_str = method_str.replace(f"fn {method}", f"pub(crate) fn {method}")
    extracted += method_str + "\n\n"
    
    content = content[:comments_start] + content[end_idx:]

with open('metanode/meta-consensus/core/src/core/proposer.rs', 'r') as f:
    proposer = f.read()

impl_idx = proposer.find('impl Core {\n')
if impl_idx != -1:
    proposer = proposer[:impl_idx + 12] + extracted + proposer[impl_idx + 12:]

with open('metanode/meta-consensus/core/src/core/proposer.rs', 'w') as f:
    f.write(proposer)
    
with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
    f.write(content)

