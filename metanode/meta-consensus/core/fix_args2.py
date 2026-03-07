import os, re
# We need to replace `Core::new` missing arguments in all tests
tests_dir = 'src/tests'
files = [f for f in os.listdir(tests_dir) if f.endswith('.rs')]
files.extend(['src/test_dag_parser.rs', 'src/core.rs', 'src/core_thread.rs', 'src/round_prober.rs', 'src/round_tracker.rs', 'src/transaction_certifier.rs'])

for file in files:
    path = os.path.join(tests_dir, file) if not file.startswith('src/') else file
    if not os.path.exists(path):
        continue
    with open(path, 'r') as f:
        content = f.read()

    # Core::new(...) missing adaptive_delay_state and system_transaction_provider
    # find lines with `Core::new` that don't have enough arguments
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

    with open(path, 'w') as f:
        f.write(content)
