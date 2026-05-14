import { createContext, useContext, useState, type ReactNode } from "react";
import { zh } from "./zh";
import { en } from "./en";

export type Lang = "zh" | "en";
export type Translations = typeof zh;

const translations: Record<Lang, Translations> = { zh, en };

interface LanguageContextType {
  lang: Lang;
  setLang: (lang: Lang) => void;
  t: Translations;
}

const LanguageContext = createContext<LanguageContextType>({
  lang: "zh",
  setLang: () => {},
  t: zh,
});

function getInitialLang(): Lang {
  try {
    const saved = localStorage.getItem("lang");
    if (saved === "en") return "en";
  } catch {}
  return "zh";
}

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(getInitialLang);

  const setLang = (newLang: Lang) => {
    setLangState(newLang);
    try {
      localStorage.setItem("lang", newLang);
    } catch {}
  };

  return (
    <LanguageContext.Provider
      value={{ lang, setLang, t: translations[lang] }}
    >
      {children}
    </LanguageContext.Provider>
  );
}

export function useTranslation() {
  return useContext(LanguageContext);
}
