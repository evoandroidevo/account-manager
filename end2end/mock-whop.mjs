// A small stand-in for Whop's API and checkout, for the end-to-end tests.
// It implements just what the account manager uses, and signs webhooks the
// way Whop does (Standard Webhooks headers, HMAC-SHA256 keyed with the
// literal bytes of the ws_ secret).
//
// Endpoints:
//   POST /checkout_configurations      (API) create a checkout, returns purchase_url
//   GET  /memberships                  (API) list memberships
//   GET  /plans/:id                    (API) a plan's prices (see PLANS)
//   POST /promo_codes                  (API) create a promo code
//   POST /memberships/:id/cancel       (API) cancel now or at period end
//   PATCH /memberships/:id             (API) { cancel_at_period_end: false }
//   GET  /checkout/:id[?promoCode=]    checkout page with the amount due and
//                                      a Pay button
//   POST /checkout/:id/pay             creates the membership, sends
//                                      membership.activated, redirects back
//   GET  /manage/:id                   stand-in for Whop's "manage" page
//   POST /test/memberships             (test control) a membership bought
//                                      without our checkout: { plan_id, email }
//   POST /test/memberships/:id/cancel  (test control) cancels and sends
//                                      membership.deactivated
//   POST /test/memberships/:id/halfway (test control) moves the billing
//                                      period so half of it is used, and
//                                      sends membership.updated
//   GET  /test/memberships/:id         (test control) the membership
//   GET  /test/promo_codes             (test control) created promo codes
//   GET  /health

import { createHmac } from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.MOCK_WHOP_PORT ?? 3011);
const BASE = `http://127.0.0.1:${PORT}`;
const API_KEY = process.env.MOCK_WHOP_API_KEY ?? "test-whop-key";
const SECRET = process.env.MOCK_WHOP_WEBHOOK_SECRET ?? "ws_test_secret";
const WEBHOOK_URL = process.env.MOCK_WHOP_WEBHOOK_URL ?? "http://127.0.0.1:3000/api/webhooks/whop";

// Prices in dollars, as Whop returns them. The first payment is
// initial_price + renewal_price.
const PLANS = {
  plan_plus: { initial_price: 0, renewal_price: 5 },
  plan_premium: { initial_price: 0, renewal_price: 15 },
};
const DAY = 86_400_000;

const checkouts = new Map();
const memberships = new Map();
const promoCodes = new Map();
let counter = 0;
const nextId = (prefix) => `${prefix}_${Date.now().toString(36)}${(counter++).toString(36)}`;

/** Sends a signed webhook to the account manager; returns its HTTP status. */
async function sendWebhook(type, membership) {
  const id = nextId("msg");
  const timestamp = Math.floor(Date.now() / 1000).toString();
  const body = JSON.stringify({
    id,
    type,
    api_version: "v1",
    timestamp: new Date().toISOString(),
    data: membership,
  });
  const signature = createHmac("sha256", Buffer.from(SECRET, "utf8"))
    .update(`${id}.${timestamp}.${body}`)
    .digest("base64");
  const response = await fetch(WEBHOOK_URL, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "webhook-id": id,
      "webhook-timestamp": timestamp,
      "webhook-signature": `v1,${signature}`,
    },
    body,
  });
  return response.status;
}

function createMembership({ plan_id, metadata = {}, email = null }) {
  const id = nextId("mem");
  const membership = {
    id,
    plan_id,
    status: "active",
    user_id: nextId("user"),
    user: email ? { email } : null,
    current_period_start: new Date().toISOString(),
    current_period_end: new Date(Date.now() + 30 * DAY).toISOString(),
    cancel_at_period_end: false,
    manage_url: `${BASE}/manage/${id}`,
    metadata,
  };
  memberships.set(id, membership);
  return membership;
}

async function readJson(request) {
  let raw = "";
  for await (const chunk of request) raw += chunk;
  return raw ? JSON.parse(raw) : {};
}

function send(response, status, body, headers = {}) {
  const isJson = typeof body !== "string";
  response.writeHead(status, {
    "content-type": isJson ? "application/json" : "text/html; charset=utf-8",
    ...headers,
  });
  response.end(isJson ? JSON.stringify(body) : body);
}

const authorized = (request) => request.headers.authorization === `Bearer ${API_KEY}`;

createServer(async (request, response) => {
  try {
    const url = new URL(request.url, BASE);
    const parts = url.pathname.split("/").filter(Boolean);

    if (url.pathname === "/health") return send(response, 200, { ok: true });

    if (request.method === "POST" && url.pathname === "/checkout_configurations") {
      if (!authorized(request)) return send(response, 401, { error: { message: "bad key" } });
      const body = await readJson(request);
      if (!body.plan_id || !body.account_id) return send(response, 400, { error: { message: "missing plan_id/account_id" } });
      const id = nextId("ch");
      checkouts.set(id, body);
      return send(response, 200, { id, purchase_url: `${BASE}/checkout/${id}`, metadata: body.metadata ?? {} });
    }

    if (request.method === "GET" && url.pathname === "/memberships") {
      if (!authorized(request)) return send(response, 401, { error: { message: "bad key" } });
      return send(response, 200, {
        data: [...memberships.values()],
        page_info: { has_next_page: false, end_cursor: null },
      });
    }

    if (request.method === "GET" && parts[0] === "plans" && parts[1]) {
      if (!authorized(request)) return send(response, 401, { error: { message: "bad key" } });
      const plan = PLANS[parts[1]];
      if (!plan) return send(response, 404, { error: { message: "no such plan" } });
      return send(response, 200, { id: parts[1], currency: "usd", ...plan });
    }

    if (request.method === "POST" && url.pathname === "/promo_codes") {
      if (!authorized(request)) return send(response, 401, { error: { message: "bad key" } });
      const body = await readJson(request);
      if (body.promo_type !== "flat_amount") return send(response, 400, { error: { message: "promo_type" } });
      if (typeof body.new_users_only !== "boolean") return send(response, 400, { error: { message: "new_users_only is required" } });
      const promo = { ...body, id: nextId("promo"), code: String(body.code).toLowerCase(), uses: 0 };
      promoCodes.set(promo.code, promo);
      return send(response, 200, promo);
    }

    if (parts[0] === "memberships" && parts[1] && (request.method === "PATCH" || parts[2] === "cancel")) {
      if (!authorized(request)) return send(response, 401, { error: { message: "bad key" } });
      const membership = memberships.get(parts[1]);
      if (!membership) return send(response, 404, { error: { message: "no such membership" } });
      const body = await readJson(request);
      if (request.method === "PATCH") {
        membership.cancel_at_period_end = Boolean(body.cancel_at_period_end);
        await sendWebhook("membership.updated", membership);
      } else if (body.cancel_at_period_end) {
        membership.cancel_at_period_end = true;
        await sendWebhook("membership.updated", membership);
      } else {
        membership.status = "canceled";
        await sendWebhook("membership.deactivated", membership);
      }
      return send(response, 200, membership);
    }

    if (parts[0] === "checkout" && parts[1] && checkouts.has(parts[1])) {
      const checkout = checkouts.get(parts[1]);
      if (request.method === "GET" && parts.length === 2) {
        const plan = PLANS[checkout.plan_id] ?? { initial_price: 0, renewal_price: 0 };
        const promo = promoCodes.get((url.searchParams.get("promoCode") ?? "").toLowerCase());
        const usable = promo && promo.uses < promo.stock && promo.plan_ids.includes(checkout.plan_id);
        const due = plan.initial_price + plan.renewal_price - (usable ? promo.amount_off : 0);
        const code = usable ? promo.code : "";
        return send(
          response,
          200,
          `<!doctype html><title>Mock Whop checkout</title>
           <h1>Mock Whop checkout</h1><p>Plan: ${checkout.plan_id}</p>
           <p data-testid="amount-due">Due today: $${due.toFixed(2)}</p>
           <form method="post" action="/checkout/${parts[1]}/pay">
             <input type="hidden" name="promo" value="${code}"><button>Pay</button>
           </form>`,
        );
      }
      if (request.method === "POST" && parts[2] === "pay") {
        let raw = "";
        for await (const chunk of request) raw += chunk;
        const promo = promoCodes.get(new URLSearchParams(raw).get("promo") ?? "");
        if (promo) promo.uses += 1;
        const membership = createMembership({ plan_id: checkout.plan_id, metadata: checkout.metadata ?? {} });
        await sendWebhook("membership.activated", membership);
        return send(response, 303, "", { location: checkout.redirect_url });
      }
    }

    if (request.method === "GET" && parts[0] === "manage") {
      return send(response, 200, `<!doctype html><title>Manage</title><h1>Manage ${parts[1]}</h1>`);
    }

    if (request.method === "POST" && url.pathname === "/test/memberships") {
      const body = await readJson(request);
      const membership = createMembership({ plan_id: body.plan_id, email: body.email ?? null });
      const webhook_status = await sendWebhook("membership.activated", membership);
      return send(response, 200, { ...membership, webhook_status });
    }

    if (request.method === "GET" && url.pathname === "/test/promo_codes") {
      return send(response, 200, [...promoCodes.values()]);
    }

    if (request.method === "GET" && parts[0] === "test" && parts[1] === "memberships" && parts[2]) {
      const membership = memberships.get(parts[2]);
      return membership ? send(response, 200, membership) : send(response, 404, { error: { message: "no such membership" } });
    }

    if (request.method === "POST" && parts[0] === "test" && parts[1] === "memberships" && parts[3] === "halfway") {
      const membership = memberships.get(parts[2]);
      if (!membership) return send(response, 404, { error: { message: "no such membership" } });
      membership.current_period_start = new Date(Date.now() - 15 * DAY).toISOString();
      membership.current_period_end = new Date(Date.now() + 15 * DAY).toISOString();
      const webhook_status = await sendWebhook("membership.updated", membership);
      return send(response, 200, { ...membership, webhook_status });
    }

    if (request.method === "POST" && parts[0] === "test" && parts[1] === "memberships" && parts[3] === "cancel") {
      const membership = memberships.get(parts[2]);
      if (!membership) return send(response, 404, { error: { message: "no such membership" } });
      membership.status = "canceled";
      const webhook_status = await sendWebhook("membership.deactivated", membership);
      return send(response, 200, { ...membership, webhook_status });
    }

    send(response, 404, { error: { message: `no route for ${request.method} ${url.pathname}` } });
  } catch (error) {
    send(response, 500, { error: { message: String(error) } });
  }
}).listen(PORT, "127.0.0.1", () => console.log(`mock Whop listening on ${BASE}`));
