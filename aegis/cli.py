import click
import os
from aegis.agents.llm_adapter import LLMGateway
from aegis.agents.gemini import GeminiProvider


@click.group()
def cli():
    """Aegis Architecture Linter & Generator"""
    pass


@cli.command()
@click.argument('path', type=click.Path(exists=True))
@click.option('--signals', is_flag=True, default=False, help='Show Ring 0.5 structural signals.')
def check(path, signals):
    """Check Ring 0 architectural rules on PATH."""
    from aegis.enforcement.validator import Ring0Enforcer
    from aegis.analysis.signals import SignalLayer

    enforcer = Ring0Enforcer()
    signal_layer = SignalLayer()
    has_violations = False

    py_files = []
    if os.path.isfile(path):
        if path.endswith('.py'):
            py_files.append(path)
    else:
        for root, dirs, files in os.walk(path):
            dirs[:] = [d for d in dirs if not d.startswith('.')]
            for file in files:
                if file.endswith('.py'):
                    py_files.append(os.path.join(root, file))

    if not py_files:
        click.echo(f"No Python files found in {path}")
        return

    for f in py_files:
        for v in enforcer.check_file(f):
            has_violations = True
            click.echo(v)

    root_dir = path if os.path.isdir(path) else os.path.dirname(path)
    for v in enforcer.check_project(py_files, root=root_dir):
        has_violations = True
        click.echo(v)

    if signals:
        click.echo("\n--- Ring 0.5 Structural Signals ---")
        for f in py_files:
            try:
                sigs = signal_layer.extract(f)
                if sigs:
                    click.echo(f"\n{f}:")
                    for sig in sigs:
                        click.echo(f"  {sig.name} = {sig.value:.0f}  ({sig.description})")
            except Exception as e:
                click.echo(f"  Warning: {f}: {e}", err=True)

    if has_violations:
        click.echo("Aegis check failed.")
        raise SystemExit(1)
    else:
        click.echo("Aegis check passed.")


@cli.command()
@click.argument('prompt')
@click.option('--output', '-o', help='Path to save the generated code.')
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
def generate(prompt, output, model):
    """Generate architecture-compliant code using a generative model."""
    try:
        provider = GeminiProvider(model_name=model)
        gateway = LLMGateway(llm_provider=provider)
        click.echo(f"Generating code via LLMGateway (Model: {model})...")
        safe_code = gateway.generate_and_validate(prompt)
        if output:
            with open(output, 'w') as f:
                f.write(safe_code)
            click.echo(f"Safely generated code written to {output}")
        else:
            click.echo("\n--- Generated Safe Code ---\n")
            click.echo(safe_code)
    except Exception as e:
        click.echo(f"Generation Failed: {e}", err=True)
        raise SystemExit(1)


@cli.command()
@click.argument('task')
@click.argument('path', type=click.Path(exists=True))
@click.option('--scope', multiple=True,
              help='Restrict patches to these paths (repeat for multiple). '
                   'Relative to PATH.')
@click.option('--max-iters', default=3, show_default=True,
              help='Max planner-executor iterations.')
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
@click.option('--no-snippets', is_flag=True, default=False,
              help='Do not send file contents to the planner.')
def refactor(task, path, scope, max_iters, model, no_snippets):
    """Run the AI refactor pipeline against PATH to accomplish TASK."""
    from aegis.runtime import pipeline

    provider = GeminiProvider(model_name=model)
    scope_list = list(scope) if scope else None
    click.echo(f"Running Aegis refactor pipeline (model={model}, max_iters={max_iters})...")
    result = pipeline.run(
        task=task,
        root=path,
        provider=provider,
        scope=scope_list,
        max_iters=max_iters,
        include_file_snippets=not no_snippets,
    )

    click.echo(f"\n--- Pipeline result ---")
    click.echo(f"success: {result.success}")
    click.echo(f"iterations: {result.iterations}")
    before = sum(len(v) for v in result.signals_before.values())
    after = sum(len(v) for v in result.signals_after.values())
    click.echo(f"signals: {before} -> {after}")
    if result.final_plan:
        click.echo(f"final plan goal: {result.final_plan.goal}")
        click.echo(f"final plan strategy: {result.final_plan.strategy}")
        click.echo(f"patches applied: {len(result.final_plan.patches)}")
    if result.validation_errors:
        click.echo("\nValidator errors (last iteration):")
        for err in result.validation_errors:
            click.echo(f"  [{err.kind}] patch={err.patch_id} edit={err.edit_index}: {err.message}")
    if result.error:
        click.echo(f"\nerror: {result.error}")
    if not result.success:
        raise SystemExit(1)


@cli.command()
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
def chat(model):
    """Start an interactive chat session with the Aegis-guarded model."""
    try:
        from prompt_toolkit import PromptSession
        from prompt_toolkit.history import InMemoryHistory

        provider = GeminiProvider(model_name=model)
        gateway = LLMGateway(llm_provider=provider)
        click.echo(f"Starting Aegis Interactive Chat (Model: {model})")
        click.echo("Type 'exit' or 'quit' to end the session.\n")
        session = PromptSession(history=InMemoryHistory())

        while True:
            try:
                user_input = session.prompt("Aegis> ")
                if user_input.lower() in ['exit', 'quit']:
                    break
                if not user_input.strip():
                    continue
                click.echo("Generating and validating...")
                response = gateway.generate_and_validate(user_input)
                click.echo(f"\n{response}\n")
            except KeyboardInterrupt:
                continue
            except EOFError:
                break
            except Exception as e:
                click.echo(f"Error: {e}", err=True)

    except ImportError:
        click.echo("Chat requires 'prompt_toolkit': pip install prompt_toolkit", err=True)
        raise SystemExit(1)
    except Exception as e:
        click.echo(f"Failed to start chat: {e}", err=True)
        raise SystemExit(1)


@cli.command('eval')
@click.option('--verbose', '-v', is_flag=True, default=False,
              help='Show extra detail (raised exceptions, event counts) per scenario.')
def run_eval(verbose):
    """Run the Aegis eval harness over the built-in scenarios.

    Each scenario asserts an ordered subsequence of DecisionTrace events.
    Exit code is 0 only when every scenario passes; CI should rely on
    this signal to catch regressions in gate behaviour as new layers
    (ToolCallValidator, policy, intent) are added.
    """
    from aegis.eval import format_results, run_all
    from aegis.eval.scenarios import SCENARIOS

    results = run_all(SCENARIOS)
    click.echo(format_results(results, verbose=verbose))

    if any(not r.passed for r in results):
        raise SystemExit(1)


if __name__ == '__main__':
    cli()
