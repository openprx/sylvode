import type { I18nText } from "./site/types";

export type Locale = "en" | "zh";

/** `?lang=zh|en` wins, then the browser language; the site content itself carries i18n maps. */
export function detectLocale(): Locale {
  try {
    const param = new URLSearchParams(window.location.search).get("lang");
    if (param === "zh" || param === "en") return param;
  } catch {
    // no window.location (never in the browser)
  }
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

const MESSAGES = {
  en: {
    "app.loading": "Loading...",
    "app.noSite":
      "No site is configured. Set VITE_SV_SITE_SLUG or open the page with ?site=<slug>.",
    "app.error": "The site could not be loaded.",
    "app.retry": "Retry",
    "app.notFound": "Page not found",
    "app.home": "Back to the home page",
    "app.mock": "Mock data",
    "nav.mine": "My requests",
    "nav.login": "Operator sign in",
    "nav.logout": "Sign out",
    "nav.operator": "Records",
    "page.operatorRequired": "This page is for site operators.",
    "page.signIn": "Sign in",
    "layout.unknownNode": "Unsupported component: {type}",
    "layout.nodeError": "This block could not be shown.",
    "layout.sourceError": "Some data could not be loaded.",
    "layout.noRows": "No rows",
    "layout.expand": "Expand",
    "layout.collapse": "Collapse",
    "layout.submit": "Submit",
    "layout.confirm": "Confirm",
    "layout.cancel": "Cancel",
    "layout.actionFailed": "The action failed.",
    "layout.notAvailable": "Not available on this site.",
    "form.required": "Required",
    "form.submitted": "Submitted",
    "form.saved": "Saved",
    "form.conflict": "The record changed meanwhile. Reload and try again.",
    "form.failed": "Could not save: {code}",
    "form.rateLimited": "Too many requests. Try again later.",
    "form.fieldNotAllowed": "This field is not allowed.",
    "mine.title": "My records",
    "mine.empty": "You have not submitted anything yet.",
    "mine.none": "This site does not list your records.",
    "login.title": "Operator sign in",
    "login.email": "Email",
    "login.password": "Password",
    "login.submit": "Sign in",
    "login.failed": "Sign-in failed.",
    "login.rate": "Too many attempts. Try again in a minute.",
    "op.title": "Records",
    "op.edit": "Edit",
    "op.save": "Save",
    "op.close": "Close",
    "op.filter": "Filter",
    "op.all": "All",
    "op.sort": "Sort",
    "op.prev": "Previous",
    "op.next": "Next",
    "op.page": "Page {page} of {pages}",
    "op.empty": "No records",
    "op.readOnly": "No fields can be updated on this binding.",
    "op.noAccess": "You have no access to this list.",
    "common.created": "Created",
    "common.updated": "Updated",
  },
  zh: {
    "app.loading": "加载中...",
    "app.noSite": "未配置站点。请设置 VITE_SV_SITE_SLUG，或使用 ?site=<slug> 打开页面。",
    "app.error": "站点加载失败。",
    "app.retry": "重试",
    "app.notFound": "页面不存在",
    "app.home": "返回首页",
    "app.mock": "模拟数据",
    "nav.mine": "我的提交",
    "nav.login": "运营登录",
    "nav.logout": "退出登录",
    "nav.operator": "记录",
    "page.operatorRequired": "此页面仅限站点运营人员。",
    "page.signIn": "登录",
    "layout.unknownNode": "不支持的组件：{type}",
    "layout.nodeError": "此区块无法显示。",
    "layout.sourceError": "部分数据无法加载。",
    "layout.noRows": "暂无数据",
    "layout.expand": "展开",
    "layout.collapse": "收起",
    "layout.submit": "提交",
    "layout.confirm": "确认",
    "layout.cancel": "取消",
    "layout.actionFailed": "操作失败。",
    "layout.notAvailable": "此站点不支持。",
    "form.required": "必填",
    "form.submitted": "已提交",
    "form.saved": "已保存",
    "form.conflict": "记录已被修改，请刷新后重试。",
    "form.failed": "保存失败：{code}",
    "form.rateLimited": "请求过多，请稍后再试。",
    "form.fieldNotAllowed": "不允许填写此字段。",
    "mine.title": "我的提交",
    "mine.empty": "你还没有提交过内容。",
    "mine.none": "此站点不展示你的记录。",
    "login.title": "运营登录",
    "login.email": "邮箱",
    "login.password": "密码",
    "login.submit": "登录",
    "login.failed": "登录失败。",
    "login.rate": "尝试次数过多，请一分钟后再试。",
    "op.title": "记录",
    "op.edit": "编辑",
    "op.save": "保存",
    "op.close": "关闭",
    "op.filter": "筛选",
    "op.all": "全部",
    "op.sort": "排序",
    "op.prev": "上一页",
    "op.next": "下一页",
    "op.page": "第 {page} / {pages} 页",
    "op.empty": "暂无记录",
    "op.readOnly": "此绑定没有可更新的字段。",
    "op.noAccess": "你无权访问此列表。",
    "common.created": "创建时间",
    "common.updated": "更新时间",
  },
} as const;

export type MessageKey = keyof (typeof MESSAGES)["en"];

export function translate(
  locale: Locale,
  key: MessageKey,
  values: Record<string, string | number> = {},
): string {
  const template: string = MESSAGES[locale][key] ?? MESSAGES.en[key];
  return template.replace(/\{(\w+)\}/g, (_all, name: string) => String(values[name] ?? ""));
}

/** An i18n map or plain text of the site content, in the given locale. */
export function pickText(value: string | I18nText | null | undefined, locale: string): string {
  if (!value) return "";
  if (typeof value === "string") return value;
  return value[locale] ?? value.en ?? Object.values(value)[0] ?? "";
}
