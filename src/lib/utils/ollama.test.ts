import { describe, it, expect } from 'vitest';
import { normalizeOllamaEndpoint } from './ollama';

describe('normalizeOllamaEndpoint', () => {
  it('returns empty string unchanged', () => {
    expect(normalizeOllamaEndpoint('')).toBe('');
  });

  it('returns whitespace-only string as empty', () => {
    expect(normalizeOllamaEndpoint('   ')).toBe('');
  });

  it('prepends http:// when no scheme is present', () => {
    expect(normalizeOllamaEndpoint('localhost:11434')).toBe('http://localhost:11434');
  });

  it('prepends http:// and adds port when only hostname given', () => {
    expect(normalizeOllamaEndpoint('localhost')).toBe('http://localhost:11434');
  });

  it('preserves http:// scheme', () => {
    expect(normalizeOllamaEndpoint('http://localhost:11434')).toBe('http://localhost:11434');
  });

  it('preserves https:// scheme', () => {
    expect(normalizeOllamaEndpoint('https://my-server.example.com:11434')).toBe(
      'https://my-server.example.com:11434'
    );
  });

  it('adds default port :11434 when scheme present but port absent', () => {
    expect(normalizeOllamaEndpoint('http://localhost')).toBe('http://localhost:11434');
  });

  it('strips trailing slash', () => {
    expect(normalizeOllamaEndpoint('http://localhost:11434/')).toBe('http://localhost:11434');
  });

  it('strips trailing slash when added by scheme-prepend + URL constructor', () => {
    expect(normalizeOllamaEndpoint('localhost/')).toBe('http://localhost:11434');
  });

  it('preserves non-default port when explicitly set', () => {
    expect(normalizeOllamaEndpoint('http://my-server:8080')).toBe('http://my-server:8080');
  });

  it('handles remote hostname without port (adds :11434)', () => {
    expect(normalizeOllamaEndpoint('http://192.168.1.100')).toBe('http://192.168.1.100:11434');
  });

  it('handles IP with port (no change)', () => {
    expect(normalizeOllamaEndpoint('http://192.168.1.100:11434')).toBe(
      'http://192.168.1.100:11434'
    );
  });

  it('trims leading/trailing whitespace before processing', () => {
    expect(normalizeOllamaEndpoint('  localhost:11434  ')).toBe('http://localhost:11434');
  });
});
