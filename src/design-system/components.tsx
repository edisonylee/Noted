import { forwardRef, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from 'react';
import { ArrowUpRight } from 'lucide-react';
import './primitives.css';

export const Button = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'accent' | 'quiet';
}>(function Button({ variant = 'primary', className = '', type = 'button', ...props }, ref) {
  return <button ref={ref} type={type} className={`nd-button nd-button--${variant} ${className}`} {...props} />;
});

export function TextField({ label, id, hint, error, ...props }: InputHTMLAttributes<HTMLInputElement> & {
  label: string; id: string; hint?: string; error?: string;
}) {
  return <label className="nd-field" htmlFor={id}>
    <span>{label}</span>
    <input {...props} id={id} aria-invalid={!!error} aria-describedby={error || hint ? `${id}-help` : undefined} />
    {(error || hint) && <small id={`${id}-help`} className={error ? 'nd-error' : ''}>{error || hint}</small>}
  </label>;
}

export function CheckField({ children, ...props }: Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> & { children: ReactNode }) {
  return <label className="nd-check"><input type="checkbox" {...props} /><span>{children}</span></label>;
}

export function Citation({ children, ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button type="button" className="nd-citation" {...props}>{children}<ArrowUpRight size={13} aria-hidden /></button>;
}
