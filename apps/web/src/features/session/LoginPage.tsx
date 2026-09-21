import { type FormEvent, useState } from "react";

import { errorMessage } from "../jobs/api";

interface LoginPageProps {
  initialError: string | null;
  onLogin: (username: string, password: string) => Promise<void>;
}

export function LoginPage({ initialError, onLogin }: LoginPageProps) {
  const [username, setUsername] = useState("owner");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(initialError);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setIsSubmitting(true);
    try {
      await onLogin(username, password);
    } catch (requestError) {
      setError(errorMessage(requestError));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <main className="login-page">
      <section className="login-card" aria-labelledby="login-title">
        <div className="section-kicker">Private dashboard</div>
        <h1 id="login-title">Sign in to TaskHarbor</h1>
        <p>Use the owner account configured on the API process.</p>
        <form onSubmit={submit}>
          <label>
            Username
            <input
              autoComplete="username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              required
            />
          </label>
          <label>
            Password
            <input
              autoComplete="current-password"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
            />
          </label>
          {error !== null && <div className="inline-alert" role="alert">{error}</div>}
          <button className="button button--primary" disabled={isSubmitting} type="submit">
            {isSubmitting ? "Signing in…" : "Sign in"}
          </button>
        </form>
      </section>
    </main>
  );
}
