import os, re
tests_dir = 'src'
files = []
for root, _, filenames in os.walk(tests_dir):
    for f in filenames:
        if f.endswith('.rs'):
            files.append(os.path.join(root, f))

for file in files:
    with open(file, 'r') as f:
        content = f.read()

    # Core::new(...) missing adaptive_delay_state and system_transaction_provider
    content = re.sub(
        r'(Core::new\([^;]+dag_state(?:\.clone\(\))?\s*,\s*(?:true|false)\s*,\s*round_tracker(?:\.clone\(\))?\s*)\)',
        r'\1, None, None)',
        content,
        flags=re.M | re.S
    )

    # CommitObserver::new(...) missing `epoch_base_index`
    content = re.sub(
        r'(CommitObserver::new\([^;]+leader_schedule(?:\.clone\(\))?\s*)\)',
        r'\1, 0)',
        content,
        flags=re.M | re.S
    )

    # telemtry substitution
    content = content.replace("telemetry_subscribers::init_for_testing();", "// telemetry_subscribers::init_for_testing();")
    content = content.replace("telemetry_subscribers::init_for_testing()", "// telemetry_subscribers::init_for_testing()")

    with open(file, 'w') as f:
        f.write(content)
