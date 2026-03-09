def update_proposer():
    with open('metanode/meta-consensus/core/src/core/proposer_methods.rs', 'r') as f:
        methods = f.read()
    
    with open('metanode/meta-consensus/core/src/core/proposer.rs', 'r') as f:
        proposer = f.read()
        
    proposer = proposer.replace('impl Core {\n}', f'impl Core {{\n{methods}\n}}')
    
    with open('metanode/meta-consensus/core/src/core/proposer.rs', 'w') as f:
        f.write(proposer)

update_proposer()
