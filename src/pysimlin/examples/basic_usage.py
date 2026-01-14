#!/usr/bin/env python3
"""Basic usage examples for the simlin Python package."""

import simlin
import pandas as pd
from pathlib import Path


def load_and_run_model():
    """Example: Load a model and run a basic simulation."""
    print("=" * 60)
    print("Example 1: Load and Run a Model")
    print("=" * 60)

    # Load a model from file
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Get information about the model
    print(f"Number of variables: {len(model.variables)}")
    print(f"Number of stocks: {len(model.stocks)}")
    print(f"Number of flows: {len(model.flows)}")

    # Run simulation using the high-level API
    run = model.run(analyze_loops=False)

    print(f"Simulation completed with {len(run.results)} time steps")
    print(f"Variables in results: {list(run.results.columns[:5])}")

    return model, run


def analyze_model_structure():
    """Example: Analyze model structure and dependencies."""
    print("\n" + "=" * 60)
    print("Example 2: Analyze Model Structure")
    print("=" * 60)

    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Get all variable names using the new API
    var_names = [v.name for v in model.variables]
    print(f"Variables in model: {', '.join(var_names[:5])}...")

    # Analyze dependencies for a variable
    if var_names:
        var = var_names[0]
        deps = model.get_incoming_links(var)
        print(f"\nDependencies for '{var}': {deps}")

    # Get all causal links
    links = model.get_links()
    print(f"\nTotal causal links: {len(links)}")

    # Display a few links
    for link in links[:3]:
        print(f"  {link}")

    return model, links


def work_with_dataframes():
    """Example: Get simulation results as pandas DataFrame."""
    print("\n" + "=" * 60)
    print("Example 3: Working with DataFrames")
    print("=" * 60)

    # Load and run a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Run simulation using high-level API
    run = model.run(analyze_loops=False)

    # Get results as DataFrame - it's already a DataFrame!
    df = run.results

    # Select a subset of variables for display
    var_names = [v.name for v in model.variables]
    selected_vars = var_names[:min(5, len(var_names))]
    df_subset = df[[v for v in selected_vars if v in df.columns]]

    print("Simulation results DataFrame:")
    print(df_subset.head())
    print(f"\nShape: {df.shape}")
    print(f"Columns: {list(df.columns[:10])}")

    # Basic analysis with pandas
    print("\nBasic statistics:")
    print(df_subset.describe())

    return df


def feedback_loop_analysis():
    """Example: Analyze feedback loops in the model."""
    print("\n" + "=" * 60)
    print("Example 4: Feedback Loop Analysis")
    print("=" * 60)

    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Get feedback loops from model
    loops = model.loops

    if loops:
        print(f"Found {len(loops)} feedback loops:")
        for loop in loops[:3]:  # Show first 3 loops
            print(f"\n{loop}")
            print(f"  Length: {len(loop.variables)} variables")
            print(f"  Polarity: {loop.polarity}")
    else:
        print("No feedback loops found in this model")

    return loops


def ltm_analysis():
    """Example: Loops That Matter analysis."""
    print("\n" + "=" * 60)
    print("Example 5: Loops That Matter (LTM) Analysis")
    print("=" * 60)

    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Run simulation with loop analysis enabled
    run = model.run(analyze_loops=True)

    # Analyze loops with behavior time series
    loops_with_behavior = [loop for loop in run.loops if loop.behavior_time_series is not None]

    if loops_with_behavior:
        print(f"Found {len(loops_with_behavior)} loops with behavior analysis")

        # Find most important loop by average importance
        most_important = max(loops_with_behavior, key=lambda l: l.average_importance() or 0)

        print(f"\nMost important loop: {most_important.id}")
        print(f"  Polarity: {most_important.polarity}")
        print(f"  Average importance: {most_important.average_importance():.4f}")
        print(f"  Max importance: {most_important.max_importance():.4f}")

        # Show dominant periods
        if run.dominant_periods:
            print(f"\nFound {len(run.dominant_periods)} dominant periods:")
            for period in run.dominant_periods[:3]:
                print(f"  t=[{period.start_time:.1f}, {period.end_time:.1f}]: {period.dominant_loops}")
    else:
        print("No loops with behavior data (model may not have feedback loops)")

    return run


def error_handling():
    """Example: Error handling and validation."""
    print("\n" + "=" * 60)
    print("Example 6: Error Handling")
    print("=" * 60)

    # Try to load an invalid model
    try:
        model = simlin.load("/nonexistent/path.stmx")
    except simlin.SimlinImportError as e:
        print(f"Import error caught: {e}")
        if e.code:
            print(f"Error code: {e.code}")

    # Check for model issues
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    issues = model.check()
    if issues:
        print(f"\nFound {len(issues)} issues in the model:")
        for issue in issues:
            print(f"  {issue.severity}: {issue.message}")
    else:
        print("\nNo issues found in the model")

    return issues


def model_conversion():
    """Example: Convert between model formats."""
    print("\n" + "=" * 60)
    print("Example 7: Model Format Conversion")
    print("=" * 60)

    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    model = simlin.load(model_path)

    # Export to XMILE via project
    project = model.project
    xmile_data = project.to_xmile()
    print(f"Exported to XMILE: {len(xmile_data)} bytes")

    # Serialize to JSON (preferred format)
    json_data = project.serialize_json()
    print(f"Serialized to JSON: {len(json_data)} bytes")

    # Serialize to protobuf (legacy format)
    pb_data = project.serialize_protobuf()
    print(f"Serialized to protobuf: {len(pb_data)} bytes")

    print("Format conversion complete")

    return xmile_data, json_data, pb_data


def main():
    """Run all examples."""
    print("Simlin Python Package - Usage Examples")
    print("=" * 60)
    
    # Check if test model exists
    model_path = Path(__file__).parent / "../tests/fixtures/eval_order.stmx"
    if not model_path.exists():
        print(f"Error: Test model not found at {model_path}")
        print("Please ensure you're running from the correct directory")
        return
    
    # Run examples
    try:
        load_and_run_model()
        analyze_model_structure()
        work_with_dataframes()
        feedback_loop_analysis()
        ltm_analysis()
        error_handling()
        model_conversion()
        
        print("\n" + "=" * 60)
        print("All examples completed successfully!")
        print("=" * 60)
        
    except Exception as e:
        print(f"\nError running examples: {e}")
        import traceback
        traceback.print_exc()


if __name__ == "__main__":
    main()