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
    project = simlin.Project.from_file(model_path)
    
    # Get information about the project
    print(f"Number of models: {len(project.get_model_names())}")
    print(f"Model names: {project.get_model_names()}")
    
    # Get the default model
    model = project.get_model()
    print(f"Number of variables: {model.get_var_count()}")
    
    # Create and run a simulation
    sim = model.new_sim()
    sim.run_to_end()
    
    print(f"Simulation completed with {sim.get_step_count()} time steps")
    
    return project, model, sim


def analyze_model_structure():
    """Example: Analyze model structure and dependencies."""
    print("\n" + "=" * 60)
    print("Example 2: Analyze Model Structure")
    print("=" * 60)
    
    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    project = simlin.Project.from_file(model_path)
    model = project.get_model()
    
    # Get all variable names
    var_names = model.get_var_names()
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
    project = simlin.Project.from_file(model_path)
    model = project.get_model()
    
    # Run simulation
    sim = model.new_sim()
    sim.run_to_end()
    
    # Get results as DataFrame
    var_names = model.get_var_names()
    # Select a subset of variables for the DataFrame
    selected_vars = var_names[:min(5, len(var_names))]
    
    df = sim.get_results(variables=selected_vars)
    
    print("Simulation results DataFrame:")
    print(df.head())
    print(f"\nShape: {df.shape}")
    print(f"Columns: {list(df.columns)}")
    
    # Basic analysis with pandas
    print("\nBasic statistics:")
    print(df.describe())
    
    return df


def feedback_loop_analysis():
    """Example: Analyze feedback loops in the model."""
    print("\n" + "=" * 60)
    print("Example 4: Feedback Loop Analysis")
    print("=" * 60)
    
    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    project = simlin.Project.from_file(model_path)
    
    # Get feedback loops
    loops = project.get_loops()
    
    if loops:
        print(f"Found {len(loops)} feedback loops:")
        for loop in loops[:3]:  # Show first 3 loops
            print(f"\n{loop}")
            print(f"  Length: {len(loop)} variables")
            print(f"  Polarity: {loop.polarity}")
    else:
        print("No feedback loops found in this model")
    
    return loops


def ltm_analysis():
    """Example: Loop Transmission Method analysis."""
    print("\n" + "=" * 60)
    print("Example 5: Loop Transmission Method (LTM) Analysis")
    print("=" * 60)
    
    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    project = simlin.Project.from_file(model_path)
    model = project.get_model()
    
    # Create simulation with LTM enabled
    sim = model.new_sim(enable_ltm=True)
    sim.run_to_end()
    
    # Get links with scores
    links = sim.get_links()
    
    # Find links with scores
    scored_links = [link for link in links if link.has_score()]
    
    if scored_links:
        print(f"Found {len(scored_links)} links with LTM scores")
        
        # Analyze top links by average score
        scored_links.sort(key=lambda x: x.average_score() or 0, reverse=True)
        
        print("\nTop 3 links by average score:")
        for link in scored_links[:3]:
            print(f"  {link.from_var} -> {link.to_var}")
            print(f"    Average score: {link.average_score():.4f}")
            print(f"    Max score: {link.max_score():.4f}")
    else:
        print("No links with LTM scores (model may not have feedback loops)")
    
    return links


def error_handling():
    """Example: Error handling and validation."""
    print("\n" + "=" * 60)
    print("Example 6: Error Handling")
    print("=" * 60)
    
    # Try to load an invalid model
    try:
        project = simlin.Project.from_xmile(b"invalid xml data")
    except simlin.SimlinImportError as e:
        print(f"Import error caught: {e}")
        if e.code:
            print(f"Error code: {e.code}")
    
    # Check for model errors
    model_path = Path("../tests/fixtures/eval_order.stmx")
    project = simlin.Project.from_file(model_path)
    
    errors = project.get_errors()
    if errors:
        print(f"\nFound {len(errors)} errors in the model:")
        for error in errors:
            print(f"  {error}")
    else:
        print("\nNo errors found in the model")
    
    return errors


def model_conversion():
    """Example: Convert between model formats."""
    print("\n" + "=" * 60)
    print("Example 7: Model Format Conversion")
    print("=" * 60)
    
    # Load a model
    model_path = Path("../tests/fixtures/eval_order.stmx")
    project = simlin.Project.from_file(model_path)
    
    # Export to XMILE
    xmile_data = project.to_xmile()
    print(f"Exported to XMILE: {len(xmile_data)} bytes")
    
    # Serialize to protobuf
    pb_data = project.serialize()
    print(f"Serialized to protobuf: {len(pb_data)} bytes")
    
    # Round-trip test
    project2 = simlin.Project.from_protobin(pb_data)
    print(f"Round-trip successful: {len(project2.get_model_names())} models")
    
    return xmile_data, pb_data


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