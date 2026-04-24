import os
from google import genai
from google.genai import types
from aegis.tools.file_system import read_file, write_file, list_directory

def test_tool_calling():
    client = genai.Client(api_key=os.environ.get("GEMINI_API_KEY"))
    
    chat = client.chats.create(
        model="gemini-2.0-flash", 
        config=types.GenerateContentConfig(
            temperature=0.0,
            tools=[read_file, write_file, list_directory]
        )
    )
    
    response = chat.send_message("Please write 'hello from tool' to a file named 'hello_tool.txt'")
    print("Response text:", response.text)
    print("Function calls:", response.function_calls)
    
    # We will need to see if it automatically calls the function, or if we have to loop.
    if os.path.exists("hello_tool.txt"):
        print("File created automatically!")
    else:
        print("File not created.")

if __name__ == "__main__":
    test_tool_calling()
