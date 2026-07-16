import OpenAI from "openai";

if (!process.env.OPENAI_API_KEY) {
  throw new Error("Set OPENAI_API_KEY before running this example.");
}

const client = new OpenAI({
  baseURL: "http://127.0.0.1:8080/v1",
  apiKey: process.env.OPENAI_API_KEY,
});

for (let i = 0; i < 2; i++) {
  const response = await client.chat.completions.create({
    model: "gpt-4o-mini",
    messages: [
      { role: "system", content: "Answer in one short sentence." },
      { role: "user", content: "What is StackIntercept testing?" },
    ],
    temperature: 0,
  });

  console.log(`request ${i + 1}: ${response.choices[0].message.content}`);
}
