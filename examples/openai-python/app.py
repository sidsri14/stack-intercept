import os

from openai import OpenAI


api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    raise SystemExit("Set OPENAI_API_KEY before running this example.")


client = OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=api_key,
)


def main() -> None:
    for i in range(2):
        response = client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {"role": "system", "content": "Answer in one short sentence."},
                {"role": "user", "content": "What is StackIntercept testing?"},
            ],
            temperature=0,
        )
        print(f"request {i + 1}: {response.choices[0].message.content}")


if __name__ == "__main__":
    main()
