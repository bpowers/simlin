#!/usr/bin/env python3
"""Generate expected CSV outputs for systems format test fixtures.

Usage: python3 scripts/gen-systems-fixtures.py

Reads .txt files from third_party/systems/examples/ and generates
CSV outputs in test/systems-format/ for integration tests.
"""
import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'third_party', 'systems'))

from systems import parse

FIXTURES = {
    'hiring': 5,
    'links': 5,
    'maximums': 5,
    'projects': 5,
    'extended_syntax': 5,
}


def generate_csv(model_name, rounds):
    src = os.path.join('third_party', 'systems', 'examples', f'{model_name}.txt')
    with open(src) as f:
        txt = f.read()
    model = parse.parse(txt)
    results = model.run(rounds=rounds)
    csv_output = model.render(results, sep=',', pad=False)
    return csv_output


def main():
    out_dir = os.path.join('test', 'systems-format')
    os.makedirs(out_dir, exist_ok=True)
    for name, rounds in FIXTURES.items():
        # Copy source .txt
        src = os.path.join('third_party', 'systems', 'examples', f'{name}.txt')
        dst_txt = os.path.join(out_dir, f'{name}.txt')
        with open(src) as f:
            txt = f.read()
        with open(dst_txt, 'w') as f:
            f.write(txt)
        # Generate CSV
        csv = generate_csv(name, rounds)
        dst_csv = os.path.join(out_dir, f'{name}_output.csv')
        with open(dst_csv, 'w') as f:
            f.write(csv)
        print(f'Generated {dst_csv} ({rounds} rounds)')


if __name__ == '__main__':
    main()
