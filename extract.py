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
        
        # Change visibility to pub(crate) if it's not already
        if "pub(crate) fn" not in method_str:
            method_str = method_str.replace(f"fn {method}", f"pub(crate) fn {method}", 1)
            
        extracted += method_str + "\n\n"
        
        # Remove from core.rs
        content = content[:comments_start] + content[end_idx:]

    # Add pub mod proposer; in core.rs
    imports_end = content.find("};", content.find("block::{")) + 2
    if "pub mod proposer;" not in content:
        content = content[:imports_end] + "\n\npub mod proposer;\n" + content[imports_end:]

    # Make fields in Core pub(crate)
    struct_start = content.find("pub(crate) struct Core {")
    struct_end = content.find("}", struct_start)
    struct_body = content[struct_start:struct_end]
    
    fields = [
        "context", "transaction_consumer", "transaction_certifier", "block_manager", 
        "propagation_delay", "committer", "last_signaled_round", "last_included_ancestors",
        "last_decided_leader", "leader_schedule", "commit_observer", "signals",
        "block_signer", "dag_state", "last_known_proposed_round", "ancestor_state_manager",
        "round_tracker", "adaptive_delay_state", "system_transaction_provider"
    ]
    
    for field in fields:
        # replace `    field:` with `    pub(crate) field:`
        struct_body = re.sub(r'(\s+)' + field + ':', r'\1pub(crate) ' + field + ':', struct_body)
        
    content = content[:struct_start] + struct_body + content[struct_end:]

    with open('metanode/meta-consensus/core/src/core/proposer.rs', 'r') as f:
        proposer = f.read()

    impl_idx = proposer.find('impl Core {')
    if impl_idx != -1:
        # Check if the block is empty or contains methods
        block_content = proposer[impl_idx + 11:]
        if 'pub(crate) fn' in block_content:
            # Assume it has been populated and rewrite it
            proposer = proposer[:impl_idx + 11] + "\n" + extracted + "}\n"
        else:
            proposer = proposer[:impl_idx + 11] + "\n" + extracted + "}\n"

    with open('metanode/meta-consensus/core/src/core/proposer.rs', 'w') as f:
        f.write(proposer)
        
    with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
        f.write(content)

extract_methods()
