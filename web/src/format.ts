const LOCALE = "en-GB";

/** an amount in the currency databricks lists its prices in */
export function money(amount: number, currency: string) {
  try {
    return new Intl.NumberFormat(LOCALE, { style: "currency", currency, maximumFractionDigits: 2 }).format(amount);
  } catch {
    // a currency code the browser does not know
    return `${amount.toFixed(2)} ${currency}`;
  }
}
