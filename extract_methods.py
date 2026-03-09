import re

def extract_methods():
    with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
        content = f.read()
    
    methods_to_extract = [
        "try_propose",
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
        # Match from "fn METHOD_NAME" up to the next outer block closing brace.
        # This regex handles simple nested braces by relying on the indentation or the next method definition,
        # but a simpler approach for these specific methods is to use a brace-counting parser.
        
        # Simple brace counting
        start_idx = content.find(f"fn {method}(")
        if start_idx == -1:
            start_idx = content.find(f"    fn {method}(")
            if start_idx == -1:
                start_idx = content.find(f"    pub(crate) fn {method}(")
        
        if start_idx == -1:
            print(f"Match not found for {method}")
            continue
            
        # backtrack to grab the doc comments and attributes
        lines = content[:start_idx].split('\n')
        comments_start = start_idx
        for i in range(len(lines) - 2, -1, -1):
            if lines[i].strip().startswith('///') or lines[i].strip().startswith('#['):
                comments_start = content.rfind(lines[i], 0, comments_start)
            else:
                break
                
        # Find the opening brace
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
        extracted += content[comments_start:end_idx] + "\n\n"
        
        # Replace the method in the original file with empty string
        content = content[:comments_start] + content[end_idx:]
        
    with open('metanode/meta-consensus/core/src/core/proposer_methods.rs', 'w') as f:
        f.write(extracted)
        
    with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
        f.write(content)

extract_methods()
