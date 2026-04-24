import pytest
import os
import tempfile
from aegis.tools.file_system import read_file, write_file, list_directory

def test_read_and_write_file():
    with tempfile.TemporaryDirectory() as tmpdir:
        test_path = os.path.join(tmpdir, "test.txt")
        
        # Write
        result = write_file(test_path, "Hello World")
        assert "Successfully wrote" in result
        
        # Read
        content = read_file(test_path)
        assert content == "Hello World"

def test_list_directory():
    with tempfile.TemporaryDirectory() as tmpdir:
        os.makedirs(os.path.join(tmpdir, "subdir"))
        with open(os.path.join(tmpdir, "file.txt"), "w") as f:
            f.write("test")
            
        result = list_directory(tmpdir)
        assert "subdir" in result
        assert "file.txt" in result
