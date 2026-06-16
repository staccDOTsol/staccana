"use client";

/**
 * Minimal toast hook + Toaster component built on @radix-ui/react-toast.
 *
 * This is a slimmed-down version of shadcn/ui's `use-toast` hook. We keep state
 * in a module-scoped reducer so any component can call `toast()` and have it
 * render in the global Toaster.
 */

import * as React from "react";

import {
  Toast,
  ToastClose,
  ToastDescription,
  ToastProvider,
  ToastTitle,
  ToastViewport,
  type ToastProps,
} from "./toast";

const TOAST_LIMIT = 3;
const TOAST_REMOVE_DELAY = 6_000;

type ToasterToast = Omit<ToastProps, "title"> & {
  id: string;
  title?: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
};

interface State {
  toasts: ToasterToast[];
}

type Action =
  | { type: "ADD_TOAST"; toast: ToasterToast }
  | { type: "DISMISS_TOAST"; toastId?: string }
  | { type: "REMOVE_TOAST"; toastId?: string };

let memoryState: State = { toasts: [] };
const listeners: Array<(state: State) => void> = [];

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "ADD_TOAST":
      return { toasts: [action.toast, ...state.toasts].slice(0, TOAST_LIMIT) };
    case "DISMISS_TOAST":
      return {
        toasts: state.toasts.map((t) =>
          action.toastId === undefined || t.id === action.toastId ? { ...t, open: false } : t,
        ),
      };
    case "REMOVE_TOAST":
      if (action.toastId === undefined) return { toasts: [] };
      return { toasts: state.toasts.filter((t) => t.id !== action.toastId) };
    default:
      return state;
  }
}

function dispatch(action: Action): void {
  memoryState = reducer(memoryState, action);
  for (const l of listeners) l(memoryState);
}

let counter = 0;
function uid(): string {
  counter = (counter + 1) % Number.MAX_SAFE_INTEGER;
  return counter.toString();
}

export interface ToastInput {
  title?: React.ReactNode;
  description?: React.ReactNode;
  variant?: ToastProps["variant"];
  action?: React.ReactNode;
  durationMs?: number;
}

export function toast(input: ToastInput): { id: string; dismiss: () => void } {
  const id = uid();
  const newToast: ToasterToast = {
    id,
    open: true,
    onOpenChange: (open) => {
      if (!open) dispatch({ type: "DISMISS_TOAST", toastId: id });
    },
    ...input,
  };
  dispatch({ type: "ADD_TOAST", toast: newToast });
  setTimeout(() => dispatch({ type: "REMOVE_TOAST", toastId: id }), input.durationMs ?? TOAST_REMOVE_DELAY);
  return { id, dismiss: () => dispatch({ type: "DISMISS_TOAST", toastId: id }) };
}

export function useToast(): { toasts: ToasterToast[]; toast: typeof toast; dismiss: (id?: string) => void } {
  const [state, setState] = React.useState<State>(memoryState);
  React.useEffect(() => {
    listeners.push(setState);
    return () => {
      const idx = listeners.indexOf(setState);
      if (idx >= 0) listeners.splice(idx, 1);
    };
  }, []);
  return {
    toasts: state.toasts,
    toast,
    dismiss: (id) => dispatch({ type: "DISMISS_TOAST", toastId: id }),
  };
}

export function Toaster(): JSX.Element {
  const { toasts } = useToast();
  return (
    <ToastProvider>
      {toasts.map(({ id, title, description, action, ...props }) => (
        <Toast key={id} {...props}>
          <div className="grid gap-1">
            {title ? <ToastTitle>{title}</ToastTitle> : null}
            {description ? <ToastDescription>{description}</ToastDescription> : null}
          </div>
          {action}
          <ToastClose />
        </Toast>
      ))}
      <ToastViewport />
    </ToastProvider>
  );
}
