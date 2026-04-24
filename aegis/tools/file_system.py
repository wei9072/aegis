import os

def read_file(path: str) -> str:
    """Reads the content of a file."""
    try:
        with open(path, 'r', encoding='utf-8') as f:
            return f.read()
    except Exception as e:
        return f"Error reading file: {e}"

def write_file(path: str, content: str) -> str:
    """Writes content to a file."""
    try:
        os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        with open(path, 'w', encoding='utf-8') as f:
            f.write(content)
        return f"Successfully wrote to {path}"
    except Exception as e:
        return f"Error writing file: {e}"

def list_directory(path: str) -> str:
    """Lists files and directories in the specified path."""
    try:
        items = os.listdir(path)
        return "\n".join(items) if items else "Directory is empty"
    except Exception as e:
        return f"Error listing directory: {e}"
