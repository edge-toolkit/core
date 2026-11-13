// The prefs `mise run gpu-probe-browser` starts each fresh profile of a browser built on Firefox with.
// They skip what a new profile opens with -- the onboarding and pre-onboarding screens, the terms-of-use and
// data-reporting notices, the welcome page and the default-browser question -- so the probe page is the only thing
// shown, let it be served over plain HTTP from localhost, which Mullvad Browser's HTTPS-Only mode would refuse, and
// write the page's console to the browser's output, which the run keeps beside the server's log.
// None of them is a fingerprinting defence, so each browser's own defences stay as it ships them.
// Firefox defines `user_pref` itself, as it reads this file into the profile.
/* global user_pref */
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.preonboarding.enabled", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.firstrunSkipsHomepage", true);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.uitour.enabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("devtools.console.stdout.content", true);
user_pref("dom.security.https_only_mode", false);
user_pref("dom.security.https_only_mode_pbm", false);
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("termsofuse.bypassNotification", true);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("trailhead.firstrun.didSeeAboutWelcome", true);
