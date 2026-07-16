# OpenAI Node SDK Example

This example points the official OpenAI Node SDK at StackIntercept.

## Setup

```bash
npm install openai
OPENAI_API_KEY="sk-your-key" node app.mjs
```

Run StackIntercept separately:

```bash
docker compose -f docker-compose.trial.yml up --build
```

Expected behavior:
- First identical request: `x-stack-intercept: miss`
- Second identical request: `x-stack-intercept: hit`

